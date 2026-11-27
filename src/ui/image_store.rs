//! Картинки на стороне окна: какие уже пришли из фонового потока, какие ещё грузятся.
//!
//! Окно само ничего не скачивает: оно просит фоновый поток (`Command::LoadImage`),
//! а тот отвечает `ImageLoaded` с байтами PNG. Декодирует PNG в текстуру egui
//! (загрузчик из egui_extras), и делает это один раз на картинку.
//!
//! Арты образов (вкладка «Обои») выгружаются из памяти, если давно не рисовались
//! (см. [`ImageStore::evict_unused`]): их тысячи, и пролистанный каталог иначе держал
//! бы в памяти гигабайты текстур. Остальные картинки (иконки, эмблемы, чемпионы,
//! предметы) мелкие и нужны постоянно — они остаются.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, TextureFilter, TextureOptions};

use crate::images::{ImageBytes, ImageKey};

/// Через сколько снова попробовать картинку, которая не загрузилась (не было сети и т.п.).
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// Арт образа, который столько не рисовался, выгружается из памяти. Понадобится
/// снова — загрузится из кэша на диске (это быстро, без сети).
const UNLOAD_AFTER: Duration = Duration::from_secs(60);

/// Как часто искать, что выгрузить.
const SWEEP_EVERY: Duration = Duration::from_secs(10);

/// Сглаживание при уменьшении: mipmap — заранее уменьшенные копии текстуры,
/// без них картинка 128 px, нарисованная в 50 px, «рябит».
const TEXTURE_OPTIONS: TextureOptions =
    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));

enum State {
    Loading,
    /// `used` — когда картинку в последний раз рисовали. `Cell` — чтобы отмечать это
    /// в [`ImageStore::get`], которому достаточно `&self`.
    Ready { bytes: Arc<[u8]>, used: Cell<Instant> },
    Failed(Instant),
}

#[derive(Default)]
pub struct ImageStore {
    states: HashMap<ImageKey, State>,
    last_sweep: Option<Instant>,
}

impl ImageStore {
    /// Какие из нужных картинок ещё не запрошены. Возвращённые сразу помечаются
    /// «грузится», поэтому каждая запрашивается один раз, хоть метод и зовётся каждый кадр.
    pub fn take_missing(&mut self, needed: impl IntoIterator<Item = ImageKey>) -> Vec<ImageKey> {
        let mut missing = Vec::new();
        for key in needed {
            let request = match self.states.get(&key) {
                None => true,
                Some(State::Failed(at)) => at.elapsed() >= RETRY_AFTER,
                Some(State::Loading | State::Ready { .. }) => false,
            };
            if request {
                self.states.insert(key, State::Loading);
                missing.push(key);
            }
        }
        missing
    }

    pub fn loaded(&mut self, key: ImageKey, bytes: ImageBytes) {
        let used = Cell::new(Instant::now());
        self.states.insert(key, State::Ready { bytes: bytes.0, used });
    }

    pub fn failed(&mut self, key: ImageKey) {
        self.states.insert(key, State::Failed(Instant::now()));
    }

    /// Готовая картинка. `None` — ещё грузится или не загрузилась (рисуем заглушку).
    /// `Image<'static>` — картинка не ссылается на временные данные: байты внутри
    /// `Arc`, а клонирование `Arc` — это +1 к счётчику ссылок, не копия байтов.
    pub fn get(&self, key: ImageKey) -> Option<egui::Image<'static>> {
        match self.states.get(&key)? {
            State::Ready { bytes, used } => {
                used.set(Instant::now());
                Some(egui::Image::from_bytes(key.uri(), Arc::clone(bytes)).texture_options(TEXTURE_OPTIONS))
            }
            State::Loading | State::Failed(_) => None,
        }
    }

    /// Выгрузить арты образов, которые не рисовались дольше [`UNLOAD_AFTER`]: и байты
    /// здесь, и всё, что egui держит по этой картинке (байты, декодированное
    /// изображение, текстуру). Зовётся каждый кадр, но проверяет раз в [`SWEEP_EVERY`].
    pub fn evict_unused(&mut self, ctx: &egui::Context, now: Instant) {
        if self.last_sweep.is_some_and(|at| now.duration_since(at) < SWEEP_EVERY) {
            return;
        }
        self.last_sweep = Some(now);
        for key in self.take_stale(now) {
            ctx.forget_image(&key.uri());
        }
    }

    /// Убрать из списка давно не рисованные арты образов; возвращает их.
    fn take_stale(&mut self, now: Instant) -> Vec<ImageKey> {
        let mut stale = Vec::new();
        self.states.retain(|key, state| {
            let unused = key.is_skin_art()
                && matches!(state, State::Ready { used, .. } if now.duration_since(used.get()) >= UNLOAD_AFTER);
            if unused {
                stale.push(*key);
            }
            !unused
        });
        stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_each_image_once() {
        let mut store = ImageStore::default();
        let icon = ImageKey::ProfileIcon(1);
        let emblem = ImageKey::Emblem(None);
        assert_eq!(store.take_missing([icon, emblem, icon]), [icon, emblem]);
        assert!(store.take_missing([icon, emblem]).is_empty(), "уже грузятся");

        store.loaded(icon, ImageBytes(Arc::from(&b"png"[..])));
        assert!(store.get(icon).is_some());
        store.failed(emblem);
        assert!(store.get(emblem).is_none());
        assert!(store.take_missing([emblem]).is_empty(), "повтор не сразу, а через минуту");
    }

    /// Давно не рисованный арт образа выгружается и при надобности запрашивается снова;
    /// недавно рисованный и мелкие картинки остаются.
    #[test]
    fn unused_skin_art_is_unloaded() {
        let mut store = ImageStore::default();
        let bytes = || ImageBytes(Arc::from(&b"jpg"[..]));
        let (old, fresh, icon) = (ImageKey::SkinThumb(1), ImageKey::SkinSplash(2), ImageKey::ProfileIcon(3));
        for key in [old, fresh, icon] {
            store.take_missing([key]);
            store.loaded(key, bytes());
        }
        let later = Instant::now() + UNLOAD_AFTER + Duration::from_secs(1);
        // Второй арт нарисовали «только что» (относительно `later`).
        if let Some(State::Ready { used, .. }) = store.states.get(&fresh) {
            used.set(later);
        }
        assert_eq!(store.take_stale(later), [old]);
        assert!(store.get(old).is_none());
        assert!(store.get(fresh).is_some() && store.get(icon).is_some());
        assert_eq!(store.take_missing([old, fresh, icon]), [old], "выгруженный арт просим заново");

        // Через egui: проверка не чаще раза в SWEEP_EVERY, и ничего не падает.
        let ctx = egui::Context::default();
        store.evict_unused(&ctx, later);
        store.evict_unused(&ctx, later + Duration::from_secs(1));
    }
}
