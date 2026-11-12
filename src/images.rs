//! Картинки для карточек: иконки профиля, эмблемы рангов и иконки чемпионов
//! (плюс русские названия чемпионов для подсказок).
//!
//! - **Иконки профиля** — из Data Dragon, официального CDN Riot со статикой игры.
//! - **Эмблемы рангов** — из CommunityDragon (raw.communitydragon.org): там выложены
//!   файлы самого клиента League. В Data Dragon эмблем нет.
//!
//! Каждая картинка скачивается один раз и хранится в кэш-папке
//! (Windows: `%LOCALAPPDATA%\lol-account-manager\images`). API-ключ туда не уходит:
//! это обычные публичные файлы.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use tokio::sync::{OnceCell, Semaphore};
use tracing::{debug, warn};

use crate::error::{AppError, AppResult};
use crate::riot::{RiotClient, Tier};
use crate::storage::{APP_DIR, DATA_DIR_ENV};

/// Эмблемы в том виде, в каком их показывает клиент League (500×500 px).
const EMBLEM_BASE_URL: &str =
    "https://raw.communitydragon.org/latest/plugins/rcp-fe-lol-shared-components/global/default";

/// Квадратные иконки чемпионов по числовому id (как в клиенте League).
const CHAMPION_ICON_BASE_URL: &str =
    "https://raw.communitydragon.org/latest/plugins/rcp-be-lol-game-data/global/default/v1/champion-icons";

/// id и русские названия всех чемпионов.
const CHAMPION_NAMES_URL: &str =
    "https://raw.communitydragon.org/latest/plugins/rcp-be-lol-game-data/global/ru_ru/v1/champion-summary.json";
/// Копия списка в кэш-папке — чтобы названия были и без сети.
const CHAMPION_NAMES_FILE: &str = "champion-summary-ru.json";

/// Оригинальные эмблемы весят ~150 КБ, а на карточке занимают ~50 px.
/// В кэш кладём уменьшенные — и места меньше, и выглядят чётче.
const EMBLEM_SIZE: u32 = 128;
/// Иконка чемпиона на строке аккаунта — ~22 px (44 px на экране с масштабом 200%).
const CHAMPION_ICON_SIZE: u32 = 64;

/// Сколько картинок скачивается одновременно.
const PARALLEL_DOWNLOADS: usize = 4;

/// Первые 8 байт любого PNG-файла.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Иконка или эмблема больше 2 МБ — явно что-то не то.
const MAX_DOWNLOAD: usize = 2 * 1024 * 1024;

/// Какая картинка нужна.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageKey {
    ProfileIcon(i32),
    /// Эмблема ранга; `None` — «без ранга».
    Emblem(Option<Tier>),
    /// Иконка чемпиона по его id.
    Champion(i32),
    /// Иконка предмета по его id (Data Dragon).
    Item(i32),
}

impl ImageKey {
    /// Имя файла в кэше.
    fn file_name(self) -> String {
        match self {
            Self::ProfileIcon(id) => format!("profile-icon-{id}.png"),
            Self::Emblem(tier) => format!("emblem-{}.png", emblem_name(tier)),
            Self::Champion(id) => format!("champion-{id}.png"),
            Self::Item(id) => format!("item-{id}.png"),
        }
    }

    /// Адрес картинки для egui: по нему egui кэширует готовую текстуру,
    /// а расширение `.png` подсказывает загрузчику формат.
    pub fn uri(self) -> String {
        format!("bytes://{}", self.file_name())
    }

    fn url(self, ddragon_version: Option<&str>) -> String {
        match self {
            Self::ProfileIcon(id) => {
                RiotClient::profile_icon_url(ddragon_version.unwrap_or_default(), id)
            }
            Self::Emblem(tier) => format!("{EMBLEM_BASE_URL}/{}.png", emblem_name(tier)),
            Self::Champion(id) => format!("{CHAMPION_ICON_BASE_URL}/{id}.png"),
            Self::Item(id) => RiotClient::item_icon_url(ddragon_version.unwrap_or_default(), id),
        }
    }
}

fn emblem_name(tier: Option<Tier>) -> String {
    // map_or(значение для None, функция для Some).
    tier.map_or_else(|| "unranked".to_string(), |t| t.api_name().to_lowercase())
}

/// Байты PNG. Обёртка нужна ради `Debug`: печатать килобайты картинки
/// в отладочный вывод незачем — печатаем только размер.
#[derive(Clone)]
pub struct ImageBytes(pub Arc<[u8]>);

impl fmt::Debug for ImageBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageBytes({} байт)", self.0.len())
    }
}

/// Папка кэша картинок. При `LOLAM_DATA_DIR` (тесты) — внутри неё.
pub fn default_cache_dir() -> PathBuf {
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) => PathBuf::from(dir).join("images"),
        None => dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR)
            .join("images"),
    }
}

/// Скачивание и кэш картинок. Клонирование дешёвое: внутри указатели.
#[derive(Clone)]
pub struct ImageCache {
    dir: PathBuf,
    http: reqwest::Client,
    /// Нужен только для версии Data Dragon (без ключа).
    riot: RiotClient,
    /// Версию Data Dragon узнаём один раз за запуск. `OnceCell` из tokio —
    /// ячейка, которую можно заполнить асинхронно; если запрос не удался,
    /// она остаётся пустой и следующая картинка попробует снова.
    ddragon_version: Arc<OnceCell<String>>,
    /// Названия чемпионов — тоже один раз за запуск (см. [`Self::champion_names`]).
    champion_names: Arc<OnceCell<Arc<HashMap<i32, String>>>>,
    slots: Arc<Semaphore>,
}

impl ImageCache {
    pub fn new(dir: PathBuf, riot: RiotClient) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("lol-account-manager/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            dir,
            http,
            riot,
            ddragon_version: Arc::default(),
            champion_names: Arc::default(),
            slots: Arc::new(Semaphore::new(PARALLEL_DOWNLOADS)),
        })
    }

    /// Русские названия чемпионов по id. Свежий список берём из сети (появляются новые
    /// чемпионы) и кладём копию на диск; без сети — читаем копию.
    pub async fn champion_names(&self) -> AppResult<Arc<HashMap<i32, String>>> {
        self.champion_names
            .get_or_try_init(|| self.load_champion_names())
            .await
            .cloned()
    }

    async fn load_champion_names(&self) -> AppResult<Arc<HashMap<i32, String>>> {
        let path = self.dir.join(CHAMPION_NAMES_FILE);
        let downloaded = async {
            let bytes = self.http.get(CHAMPION_NAMES_URL).send().await?.error_for_status()?.bytes().await?;
            if bytes.len() > MAX_DOWNLOAD {
                return Err(AppError::Decode("список чемпионов слишком большой".into()));
            }
            // Сначала разбираем: испорченный ответ не должен затереть хорошую копию.
            let names = parse_champion_names(&bytes)?;
            if let Err(e) = save_atomically(&self.dir, &path, &bytes).await {
                warn!(error = %e, "не удалось сохранить список чемпионов");
            }
            Ok(names)
        }
        .await;
        match downloaded {
            Ok(names) => Ok(Arc::new(names)),
            Err(e) => {
                debug!(error = %e, "список чемпионов не скачался — берём копию с диска");
                let bytes = tokio::fs::read(&path).await.map_err(|_| e)?;
                Ok(Arc::new(parse_champion_names(&bytes)?))
            }
        }
    }

    /// PNG-байты картинки: из кэша, а если там нет — скачать и сохранить.
    pub async fn load(&self, key: ImageKey) -> AppResult<ImageBytes> {
        let path = self.dir.join(key.file_name());

        // 1. Уже в кэше? Испорченный файл (не PNG) просто скачаем заново.
        if let Ok(bytes) = tokio::fs::read(&path).await
            && bytes.starts_with(PNG_SIGNATURE)
        {
            return Ok(ImageBytes(bytes.into()));
        }

        // 2. Скачать. Пропуск семафора держим до конца функции (`_slot`).
        let _slot = self.slots.acquire().await.ok();
        let version = match key {
            ImageKey::ProfileIcon(_) | ImageKey::Item(_) => Some(
                self.ddragon_version
                    .get_or_try_init(|| self.riot.ddragon_latest_version())
                    .await?
                    .as_str(),
            ),
            ImageKey::Emblem(_) | ImageKey::Champion(_) => None,
        };
        let url = key.url(version);
        debug!(%url, "скачиваем картинку");
        let response = self.http.get(&url).send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(AppError::Image("её нет на сервере".into()));
        }
        let bytes = response.error_for_status()?.bytes().await?;
        if bytes.len() > MAX_DOWNLOAD || !bytes.starts_with(PNG_SIGNATURE) {
            return Err(AppError::Image("сервер прислал не PNG".into()));
        }

        // 3. Эмблемы уменьшаем. Декодирование и масштабирование — работа для
        //    процессора на десятки миллисекунд, поэтому в пуле блокирующих задач.
        let shrink_to = match key {
            ImageKey::Emblem(_) => Some(EMBLEM_SIZE),
            ImageKey::Champion(_) => Some(CHAMPION_ICON_SIZE),
            ImageKey::ProfileIcon(_) | ImageKey::Item(_) => None,
        };
        let bytes: Vec<u8> = match shrink_to {
            Some(size) => tokio::task::spawn_blocking(move || shrink_png(&bytes, size))
                .await
                .map_err(|_| AppError::Internal("уменьшение картинки аварийно завершилось".into()))??,
            None => bytes.to_vec(),
        };

        // 4. В кэш. Не получилось — не страшно: в следующий раз скачаем снова.
        if let Err(e) = save_atomically(&self.dir, &path, &bytes).await {
            warn!(error = %e, file = %path.display(), "не удалось сохранить картинку в кэш");
        }
        Ok(ImageBytes(bytes.into()))
    }
}

/// `champion-summary.json` → id → название. id -1 — служебная запись «нет чемпиона».
fn parse_champion_names(bytes: &[u8]) -> AppResult<HashMap<i32, String>> {
    #[derive(serde::Deserialize)]
    struct Champion {
        id: i32,
        name: String,
    }
    let list: Vec<Champion> = serde_json::from_slice(bytes).map_err(|e| AppError::Decode(e.to_string()))?;
    Ok(list.into_iter().filter(|c| c.id > 0).map(|c| (c.id, c.name)).collect())
}

/// Пиксели прозрачнее этого считаем пустыми полями (слабое свечение вокруг эмблемы).
const ALPHA_THRESHOLD: u8 = 24;

/// Обрезать прозрачные поля (оставив квадрат) и уменьшить до `size`×`size`.
fn shrink_png(bytes: &[u8], size: u32) -> AppResult<Vec<u8>> {
    let image_error = |e: image::ImageError| AppError::Image(e.to_string());
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).map_err(image_error)?;
    let (x, y, side) = square_content_bounds(&image.to_rgba8());
    let image = image.crop_imm(x, y, side, side);
    // Lanczos3 — качественный фильтр: мелкие детали эмблемы не превращаются в «лесенку».
    let small = image.resize(size, size, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(image_error)?;
    Ok(out)
}

/// Квадрат вокруг непрозрачной части картинки: (x, y, сторона).
/// Квадрат — потому что egui растягивает картинку на всю отведённую клетку,
/// а клетка у эмблемы квадратная: неквадратная картинка исказилась бы.
fn square_content_bounds(rgba: &image::RgbaImage) -> (u32, u32, u32) {
    let (width, height) = rgba.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel[3] > ALPHA_THRESHOLD {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 || y0 > y1 {
        return (0, 0, width.min(height)); // картинка целиком прозрачная — не трогаем
    }
    // Сторона — по большему из размеров содержимого, но не больше самой картинки.
    let side = (x1 - x0 + 1).max(y1 - y0 + 1).min(width).min(height);
    // Центрируем квадрат на содержимом и сдвигаем внутрь картинки, если вылез за край.
    // saturating_sub — вычитание без ухода ниже нуля (у u32 отрицательных чисел нет).
    let center = |lo: u32, hi: u32| (lo + hi) / 2;
    let x = center(x0, x1).saturating_sub(side / 2).min(width - side);
    let y = center(y0, y1).saturating_sub(side / 2).min(height - side);
    (x, y, side)
}

/// Сначала во временный файл, потом переименовать: если программа закроется посреди
/// записи, в кэше не останется обрезанной картинки.
async fn save_atomically(dir: &Path, path: &Path, bytes: &[u8]) -> AppResult<()> {
    tokio::fs::create_dir_all(dir).await?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    tokio::fs::write(&tmp, bytes).await?;
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_files_and_urls() {
        assert_eq!(ImageKey::ProfileIcon(588).uri(), "bytes://profile-icon-588.png");
        assert_eq!(
            ImageKey::ProfileIcon(588).url(Some("16.21.1")),
            "https://ddragon.leagueoflegends.com/cdn/16.21.1/img/profileicon/588.png"
        );
        assert_eq!(ImageKey::Emblem(Some(Tier::Grandmaster)).file_name(), "emblem-grandmaster.png");
        assert_eq!(ImageKey::Emblem(None).file_name(), "emblem-unranked.png");
        assert!(
            ImageKey::Emblem(Some(Tier::Gold))
                .url(None)
                .ends_with("/global/default/gold.png")
        );
        assert_eq!(ImageKey::Champion(42).file_name(), "champion-42.png");
        assert_eq!(
            ImageKey::Item(3031).url(Some("16.22.1")),
            "https://ddragon.leagueoflegends.com/cdn/16.22.1/img/item/3031.png"
        );
        assert!(ImageKey::Champion(42).url(None).ends_with("/v1/champion-icons/42.png"));
    }

    #[test]
    fn parses_champion_names() {
        let json = r#"[{"id": -1, "name": "Нет", "alias": "None"},
                       {"id": 42, "name": "Корки", "alias": "Corki", "squarePortraitPath": "/x.png"}]"#;
        let names = parse_champion_names(json.as_bytes()).unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[&42], "Корки");
        assert!(parse_champion_names(b"not json").is_err());
    }

    #[test]
    fn shrinks_png_and_debug_hides_bytes() {
        // Картинка 300×300 → 128×128, на выходе снова PNG.
        let big = image::RgbaImage::from_pixel(300, 300, image::Rgba([200, 170, 110, 255]));
        let mut png = Vec::new();
        image::DynamicImage::from(big)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let small = shrink_png(&png, 128).unwrap();
        assert!(small.starts_with(PNG_SIGNATURE));
        let decoded = image::load_from_memory(&small).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (128, 128));

        let debug = format!("{:?}", ImageBytes(small.into()));
        assert!(debug.starts_with("ImageBytes(") && debug.ends_with(" байт)"), "{debug}");
    }

    #[test]
    fn crops_transparent_margins_to_a_square() {
        // Картинка 500×500, непрозрачная только полоса 200×100 в точке (150, 250).
        let mut img = image::RgbaImage::new(500, 500);
        for x in 150..350 {
            for y in 250..350 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        // Квадрат 200×200 с центром в центре полосы (249, 299).
        assert_eq!(square_content_bounds(&img), (149, 199, 200));
        // Содержимое у края: квадрат сдвигается внутрь картинки.
        let mut edge = image::RgbaImage::new(100, 100);
        for x in 0..100 {
            edge.put_pixel(x, 99, image::Rgba([255, 255, 255, 255]));
        }
        assert_eq!(square_content_bounds(&edge), (0, 0, 100));
        // Полностью прозрачная — без изменений.
        assert_eq!(square_content_bounds(&image::RgbaImage::new(40, 30)), (0, 0, 30));
    }

    /// Настоящая загрузка из сети во временную папку.
    #[tokio::test]
    #[ignore = "нужен интернет"]
    async fn downloads_icon_and_emblem_live() {
        let dir = std::env::temp_dir().join(format!("lolam-images-{}", uuid::Uuid::new_v4().simple()));
        let cache = ImageCache::new(dir.clone(), RiotClient::new().unwrap()).unwrap();
        for key in [ImageKey::ProfileIcon(29), ImageKey::Emblem(Some(Tier::Gold)), ImageKey::Emblem(None)] {
            let bytes = cache.load(key).await.unwrap();
            assert!(bytes.0.starts_with(PNG_SIGNATURE), "{key:?}");
            assert!(dir.join(key.file_name()).is_file(), "{key:?} в кэше");
        }
        // Второй раз — из кэша (тот же результат).
        let again = cache.load(ImageKey::Emblem(Some(Tier::Gold))).await.unwrap();
        assert!(again.0.starts_with(PNG_SIGNATURE));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
