//! Обновление приложения из GitHub Releases.
//!
//! Как это работает:
//! 1. спрашиваем у GitHub последний выпуск (обычный публичный адрес, без ключей);
//! 2. если его версия новее нашей — показываем это в окне;
//! 3. по кнопке скачиваем установщик, сверяем его SHA-256 с файлом `SHA256SUMS.txt`
//!    из того же выпуска и запускаем; установщик ставит новую версию поверх старой
//!    и снова открывает приложение.
//!
//! Безопасность: скачиваем только по HTTPS и только из выпусков нашего репозитория
//! (адрес проверяется), имя файла — только ожидаемого вида, а файл, у которого не
//! сошлась контрольная сумма, удаляется и не запускается. В запросах нет ничего
//! о пользователе и его аккаунтах: только название и версия программы (User-Agent).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use crate::error::{AppError, AppResult};
use crate::storage::{APP_DIR, DATA_DIR_ENV};

/// Последний выпуск (черновики и предварительные версии GitHub сюда не отдаёт).
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/minibgg/lol-account-manager/releases/latest";
/// Страница выпусков — для кнопки «Что нового» и ручного скачивания.
pub const RELEASES_PAGE: &str = "https://github.com/minibgg/lol-account-manager/releases/latest";
/// Страницы нашего репозитория (ссылку «Что нового» открываем, только если она отсюда).
const REPO_PREFIX: &str = "https://github.com/minibgg/lol-account-manager/";
/// Файлы выпусков нашего репозитория. Что-либо с другого адреса не скачиваем.
const DOWNLOAD_PREFIX: &str = "https://github.com/minibgg/lol-account-manager/releases/download/";

/// Имя установщика в выпуске: `LoL-Account-Manager-Setup-1.2.3.exe` (см. release.yml).
const INSTALLER_PREFIX: &str = "LoL-Account-Manager-Setup-";
const SUMS_FILE: &str = "SHA256SUMS.txt";

/// Папка, куда ставит установщик (`%LOCALAPPDATA%\Programs\…`, см. release.yml и examples/install.rs).
const INSTALL_DIR_NAME: &str = "LoL Account Manager";

/// Установщик весит ~10 МБ; больше 300 МБ — явно что-то не то.
const MAX_INSTALLER_SIZE: u64 = 300 * 1024 * 1024;
const MAX_METADATA_SIZE: usize = 1024 * 1024;

/// Версия `1.2.3`.
///
/// `derive(PartialOrd, Ord)` сравнивает поля по порядку объявления — сначала major,
/// потом minor, потом patch. Ровно то, что нужно для версий.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// «1.2.3» или «v1.2.3». Всё остальное (в том числе «1.2.3-beta») — `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        // `?` работает и с Option: None сразу возвращается из функции.
        let mut next = || parts.next()?.parse::<u32>().ok();
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        // Четвёртой части быть не должно.
        parts.next().is_none().then_some(version)
    }

    /// Версия этой программы (из Cargo.toml, вшита при компиляции).
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("версия в Cargo.toml — три числа через точку")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Найденная новая версия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: Version,
    /// Страница выпуска на GitHub («Что нового»).
    pub page_url: String,
    /// Установщик; `None` — в выпуске его нет (тогда только ссылка на страницу).
    pub installer: Option<Installer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installer {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Адрес файла SHA256SUMS.txt того же выпуска.
    pub sums_url: String,
}

/// Ответ GitHub — только нужные поля, остальные serde пропускает.
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Разбирает ответ GitHub. `Ok(None)` — новее, чем `current`, ничего нет.
pub fn pick_update(json: &[u8], current: Version) -> AppResult<Option<UpdateInfo>> {
    let release: Release = serde_json::from_slice(json).map_err(|e| AppError::Decode(e.to_string()))?;
    let Some(version) = Version::parse(&release.tag_name) else {
        return Err(AppError::Decode(format!("непонятная версия «{}»", release.tag_name)));
    };
    if version <= current {
        return Ok(None);
    }
    let asset_url = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name && a.browser_download_url.starts_with(DOWNLOAD_PREFIX))
    };
    let installer_name = format!("{INSTALLER_PREFIX}{version}.exe");
    // Установщик берём, только если рядом лежат контрольные суммы: без них не проверить файл.
    let installer = match (asset_url(&installer_name), asset_url(SUMS_FILE)) {
        (Some(file), Some(sums)) if file.size > 0 && file.size <= MAX_INSTALLER_SIZE => Some(Installer {
            name: installer_name,
            url: file.browser_download_url.clone(),
            size: file.size,
            sums_url: sums.browser_download_url.clone(),
        }),
        _ => None,
    };
    let page_url = if release.html_url.starts_with(REPO_PREFIX) {
        release.html_url
    } else {
        RELEASES_PAGE.to_string()
    };
    Ok(Some(UpdateInfo {
        version,
        page_url,
        installer,
    }))
}

/// Ищет в SHA256SUMS.txt сумму файла. Строки вида `<64 hex-цифры>  <имя файла>`.
pub fn expected_sha256(sums: &str, file_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        // Перед именем может стоять `*` («двоичный режим» в формате sha256sum).
        let name = name.trim().trim_start_matches('*');
        let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (valid && name == file_name).then(|| hash.to_ascii_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Папка для скачанного установщика. При `LOLAM_DATA_DIR` (тесты) — внутри неё.
pub fn default_dir() -> PathBuf {
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) => PathBuf::from(dir).join("updates"),
        None => dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR)
            .join("updates"),
    }
}

/// Можно ли обновиться «одной кнопкой»: только на Windows и только если программа
/// запущена из папки, куда её ставит установщик. Переносную копию (или сборку из
/// исходников) установщик не заменит — для них показываем ссылку на страницу выпуска.
pub fn can_self_install() -> bool {
    if !cfg!(windows) {
        return false;
    }
    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    let install_dir = dirs::data_local_dir().map(|dir| dir.join("Programs").join(INSTALL_DIR_NAME));
    match (exe_dir, install_dir) {
        // В Windows регистр букв в путях не важен.
        (Some(exe), Some(install)) => {
            exe.to_string_lossy().to_lowercase() == install.to_string_lossy().to_lowercase()
        }
        _ => false,
    }
}

/// Запускает скачанный установщик. Сразу после этого приложение должно закрыться,
/// чтобы установщик смог заменить exe-файл.
///
/// - `/SILENT` — без вопросов, видно только окошко с полосой установки;
/// - `/SUPPRESSMSGBOXES` — не показывать окна-сообщения;
/// - `/relaunch=1` — наш параметр (см. release.yml): открыть приложение после установки.
pub fn launch_installer(path: &Path) -> AppResult<()> {
    std::process::Command::new(path)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/relaunch=1"])
        .spawn()
        .map(drop)
        .map_err(|e| AppError::UpdateLaunch(e.to_string()))
}

/// Проверка и скачивание обновлений. Клонирование дешёвое.
#[derive(Clone)]
pub struct Updater {
    http: reqwest::Client,
    dir: PathBuf,
}

impl Updater {
    pub fn new(dir: PathBuf) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            // GitHub требует User-Agent. В нём только название и версия программы.
            .user_agent(concat!("lol-account-manager/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { http, dir })
    }

    /// Есть ли версия новее `current`.
    pub async fn check(&self, current: Version) -> AppResult<Option<UpdateInfo>> {
        let response = self
            .http
            .get(LATEST_RELEASE_API)
            .header("Accept", "application/vnd.github+json")
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        let status = response.status().as_u16();
        match status {
            200 => {}
            // Выпусков ещё нет.
            404 => return Ok(None),
            other => return Err(AppError::UnexpectedStatus(other)),
        }
        let body = response.bytes().await?;
        if body.len() > MAX_METADATA_SIZE {
            return Err(AppError::Decode("слишком большой ответ GitHub".into()));
        }
        let update = pick_update(&body, current)?;
        debug!(?update, "проверка обновлений");
        Ok(update)
    }

    /// Скачивает установщик и сверяет контрольную сумму. Возвращает путь к файлу.
    /// `progress(получено, всего)` вызывается по мере скачивания.
    pub async fn download(
        &self,
        installer: &Installer,
        mut progress: impl FnMut(u64, u64),
    ) -> AppResult<PathBuf> {
        check_installer(installer)?;

        // 1. Контрольная сумма из того же выпуска.
        let sums = self
            .http
            .get(&installer.sums_url)
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        if sums.len() > MAX_METADATA_SIZE {
            return Err(AppError::UpdateRejected("файл контрольных сумм слишком большой".into()));
        }
        let expected = expected_sha256(&String::from_utf8_lossy(&sums), &installer.name)
            .ok_or_else(|| AppError::UpdateRejected("в выпуске нет контрольной суммы установщика".into()))?;

        // 2. Сам файл — по частям, чтобы не держать его целиком в памяти и показывать ход.
        // Старые загрузки убираем: папка нужна только для одного установщика.
        let _ = tokio::fs::remove_dir_all(&self.dir).await;
        tokio::fs::create_dir_all(&self.dir).await?;
        let part = self.dir.join(format!("{}.part", installer.name));
        let target = self.dir.join(&installer.name);

        let mut response = self
            .http
            .get(&installer.url)
            .timeout(Duration::from_secs(15 * 60))
            .send()
            .await?
            .error_for_status()?;
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut received: u64 = 0;
        progress(0, installer.size);
        // `chunk()` отдаёт очередной кусок ответа; None — ответ закончился.
        while let Some(chunk) = response.chunk().await? {
            received += chunk.len() as u64;
            if received > installer.size {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return Err(AppError::UpdateRejected("файл больше, чем заявлено в выпуске".into()));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            progress(received, installer.size);
        }
        file.flush().await?;
        drop(file); // закрыть файл, иначе Windows не даст его переименовать и запустить

        // 3. Проверка.
        let actual = hex(&hasher.finalize());
        if received != installer.size || actual != expected {
            let _ = tokio::fs::remove_file(&part).await;
            let reason = if received != installer.size {
                "файл скачался не полностью"
            } else {
                "контрольная сумма не совпала"
            };
            return Err(AppError::UpdateRejected(reason.into()));
        }
        tokio::fs::rename(&part, &target).await?;
        info!(file = %target.display(), "обновление скачано и проверено");
        Ok(target)
    }

    /// Удаляет скачанные установщики (после обновления они больше не нужны).
    pub async fn clean(&self) {
        let _ = tokio::fs::remove_dir_all(&self.dir).await;
    }
}

/// Данные об установщике могли прийти откуда угодно — проверяем ещё раз перед скачиванием.
fn check_installer(installer: &Installer) -> AppResult<()> {
    let name_ok = installer.name.starts_with(INSTALLER_PREFIX)
        && installer.name.ends_with(".exe")
        // Только буквы, цифры, точка, дефис: никаких `..`, `/` и `\` в имени файла.
        && installer.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    let urls_ok = installer.url.starts_with(DOWNLOAD_PREFIX) && installer.sums_url.starts_with(DOWNLOAD_PREFIX);
    let size_ok = installer.size > 0 && installer.size <= MAX_INSTALLER_SIZE;
    if name_ok && urls_ok && size_ok {
        Ok(())
    } else {
        Err(AppError::UpdateRejected("неожиданные данные о выпуске".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V010: Version = Version { major: 0, minor: 1, patch: 0 };

    fn release_json(tag: &str, assets: &[(&str, &str, u64)]) -> Vec<u8> {
        let assets: Vec<_> = assets
            .iter()
            .map(|(name, url, size)| serde_json::json!({ "name": name, "browser_download_url": url, "size": size }))
            .collect();
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("{REPO_PREFIX}releases/tag/{tag}"),
            "name": "что-то ещё, что нам не нужно",
            "assets": assets,
        })
        .to_string()
        .into_bytes()
    }

    fn url(tag: &str, file: &str) -> String {
        format!("{DOWNLOAD_PREFIX}{tag}/{file}")
    }

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version { major: 1, minor: 2, patch: 3 }));
        assert_eq!(Version::parse("0.10.0"), Some(Version { major: 0, minor: 10, patch: 0 }));
        for bad in ["", "1.2", "1.2.3.4", "1.2.x", "v1.2.3-beta", "-1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        // 0.10.0 новее 0.9.9 — сравниваем числа, а не строки.
        assert!(Version::parse("0.10.0") > Version::parse("0.9.9"));
        assert!(Version::parse("1.0.0") > Version::parse("0.99.99"));
        assert_eq!(Version::parse("1.2.3").unwrap().to_string(), "1.2.3");
        // Версия самой программы разбирается (иначе current() паникует).
        let _ = Version::current();
    }

    #[test]
    fn newer_release_with_installer_is_offered() {
        let json = release_json(
            "v0.2.0",
            &[
                ("LoL-Account-Manager-Portable-0.2.0.exe", &url("v0.2.0", "LoL-Account-Manager-Portable-0.2.0.exe"), 9_000_000),
                ("LoL-Account-Manager-Setup-0.2.0.exe", &url("v0.2.0", "LoL-Account-Manager-Setup-0.2.0.exe"), 5_000_000),
                ("SHA256SUMS.txt", &url("v0.2.0", "SHA256SUMS.txt"), 200),
            ],
        );
        let update = pick_update(&json, V010).unwrap().expect("0.2.0 новее 0.1.0");
        assert_eq!(update.version.to_string(), "0.2.0");
        assert!(update.page_url.ends_with("/releases/tag/v0.2.0"));
        let installer = update.installer.expect("установщик есть");
        assert_eq!(installer.name, "LoL-Account-Manager-Setup-0.2.0.exe");
        assert_eq!(installer.size, 5_000_000);
        assert!(installer.sums_url.ends_with("/SHA256SUMS.txt"));
        assert!(check_installer(&installer).is_ok());
    }

    #[test]
    fn same_or_older_release_is_not_an_update() {
        assert_eq!(pick_update(&release_json("v0.1.0", &[]), V010).unwrap(), None);
        assert_eq!(pick_update(&release_json("v0.0.9", &[]), V010).unwrap(), None);
        assert!(pick_update(&release_json("nightly", &[]), V010).is_err());
        assert!(pick_update(b"<html>", V010).is_err());
    }

    #[test]
    fn installer_from_elsewhere_or_without_sums_is_ignored() {
        let name = "LoL-Account-Manager-Setup-0.2.0.exe";
        // Чужой адрес файла.
        let foreign = release_json(
            "v0.2.0",
            &[
                (name, "https://example.com/setup.exe", 5_000_000),
                ("SHA256SUMS.txt", &url("v0.2.0", "SHA256SUMS.txt"), 200),
            ],
        );
        // Нет контрольных сумм.
        let no_sums = release_json("v0.2.0", &[(name, &url("v0.2.0", name), 5_000_000)]);
        // Подозрительно большой файл.
        let huge = release_json(
            "v0.2.0",
            &[
                (name, &url("v0.2.0", name), MAX_INSTALLER_SIZE + 1),
                ("SHA256SUMS.txt", &url("v0.2.0", "SHA256SUMS.txt"), 200),
            ],
        );
        for json in [foreign, no_sums, huge] {
            let update = pick_update(&json, V010).unwrap().expect("версия новее");
            assert_eq!(update.installer, None, "о версии сообщаем, но сами не ставим");
        }
    }

    #[test]
    fn installer_data_is_rechecked_before_download() {
        let good = Installer {
            name: "LoL-Account-Manager-Setup-0.2.0.exe".into(),
            url: url("v0.2.0", "LoL-Account-Manager-Setup-0.2.0.exe"),
            size: 5_000_000,
            sums_url: url("v0.2.0", "SHA256SUMS.txt"),
        };
        assert!(check_installer(&good).is_ok());
        let bad_name = Installer { name: r"LoL-Account-Manager-Setup-..\..\evil.exe".into(), ..good.clone() };
        let bad_url = Installer { url: "https://github.com/someone/else/releases/download/v1/a.exe".into(), ..good.clone() };
        let bad_sums = Installer { sums_url: "http://example.com/SHA256SUMS.txt".into(), ..good.clone() };
        let empty = Installer { size: 0, ..good };
        for bad in [bad_name, bad_url, bad_sums, empty] {
            assert!(matches!(check_installer(&bad), Err(AppError::UpdateRejected(_))), "{bad:?}");
        }
    }

    #[test]
    fn finds_checksum_by_file_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  LoL-Account-Manager-Portable-0.2.0.exe\r\n{b}  LoL-Account-Manager-Setup-0.2.0.exe\r\n");
        // Регистр суммы приводим к нижнему: так её печатает наш `hex`.
        assert_eq!(expected_sha256(&sums, "LoL-Account-Manager-Setup-0.2.0.exe"), Some("b".repeat(64)));
        assert_eq!(expected_sha256(&sums, "LoL-Account-Manager-Portable-0.2.0.exe"), Some(a.clone()));
        assert_eq!(expected_sha256(&sums, "other.exe"), None);
        assert_eq!(expected_sha256(&format!("{a} *file.exe"), "file.exe"), Some(a));
        // Не сумма (короткая или не hex) — не подходит.
        assert_eq!(expected_sha256("abc  file.exe", "file.exe"), None);
        assert_eq!(expected_sha256(&format!("{}  file.exe", "z".repeat(64)), "file.exe"), None);
    }

    #[test]
    fn sha256_hex_matches_known_value() {
        // Известное значение: SHA-256 от строки «abc».
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
