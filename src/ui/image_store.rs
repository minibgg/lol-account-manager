//! Картинки на стороне окна: какие уже пришли из фонового потока, какие ещё грузятся.
//!
//! Окно само ничего не скачивает: оно просит фоновый поток (`Command::LoadImage`),
//! а тот отвечает `ImageLoaded` с байтами PNG. Декодирует PNG в текстуру egui
//! (загрузчик из egui_extras), и делает это один раз на картинку.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, TextureFilter, TextureOptions};

use crate::images::{ImageBytes, ImageKey};

/// Через сколько снова попробовать картинку, которая не загрузилась (не было сети и т.п.).
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// Сглаживание при уменьшении: mipmap — заранее уменьшенные копии текстуры,
/// без них картинка 128 px, нарисованная в 50 px, «рябит».
const TEXTURE_OPTIONS: TextureOptions =
    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));

enum State {
    Loading,
    Ready(Arc<[u8]>),
    Failed(Instant),
}

#[derive(Default)]
pub struct ImageStore {
    states: HashMap<ImageKey, State>,
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
                Some(State::Loading | State::Ready(_)) => false,
            };
            if request {
                self.states.insert(key, State::Loading);
                missing.push(key);
            }
        }
        missing
    }

    pub fn loaded(&mut self, key: ImageKey, bytes: ImageBytes) {
        self.states.insert(key, State::Ready(bytes.0));
    }

    pub fn failed(&mut self, key: ImageKey) {
        self.states.insert(key, State::Failed(Instant::now()));
    }

    /// Готовая картинка. `None` — ещё грузится или не загрузилась (рисуем заглушку).
    /// `Image<'static>` — картинка не ссылается на временные данные: байты внутри
    /// `Arc`, а клонирование `Arc` — это +1 к счётчику ссылок, не копия байтов.
    pub fn get(&self, key: ImageKey) -> Option<egui::Image<'static>> {
        match self.states.get(&key)? {
            State::Ready(bytes) => Some(
                egui::Image::from_bytes(key.uri(), Arc::clone(bytes)).texture_options(TEXTURE_OPTIONS),
            ),
            State::Loading | State::Failed(_) => None,
        }
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
}
