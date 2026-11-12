//! Буфер обмена для логинов и паролей.
//!
//! - На Windows скопированное не попадает в журнал буфера (Win+V) и в облачную
//!   синхронизацию, на Linux — в историю менеджеров буфера (KDE Klipper и др.).
//! - Через 30 секунд буфер очищается, но только если там всё ещё наш секрет:
//!   если пользователь успел скопировать что-то своё, мы это не трогаем.
//! - Сам секрет после копирования не хранится: для сравнения запоминаем только
//!   его отпечаток (хеш со случайным ключом).
//!
//! Все методы блокирующие (системный буфер — синхронный API), поэтому backend
//! вызывает их через `spawn_blocking`.

use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::error::{AppError, AppResult};

/// Через сколько стирать скопированный логин или пароль.
pub const CLEAR_AFTER: Duration = Duration::from_secs(30);

/// Разделяемый буфер обмена.
///
/// `Arc<Mutex<...>>` — типичная пара для общих изменяемых данных между потоками:
/// - `Arc` (atomic reference counter) — несколько владельцев одного значения;
///   `clone()` копирует только указатель и увеличивает счётчик;
/// - `Mutex` — «замок»: менять данные может только тот, кто вызвал `lock()`,
///   остальные ждут. Замок снимается сам, когда guard из `lock()` уничтожается.
#[derive(Clone, Default)]
pub struct SecretClipboard {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    /// Создаётся при первом копировании и живёт до выхода из программы:
    /// на Linux (X11) содержимое буфера доступно, только пока жив этот объект.
    clipboard: Option<arboard::Clipboard>,
    /// Отпечаток последнего скопированного секрета (None — в буфере ничего нашего).
    fingerprint: Option<u64>,
    /// Номер последнего копирования. Таймер от старого копирования не должен
    /// стирать новое: он сверяет номер.
    generation: u64,
    /// Случайный ключ хеширования, свой у каждого запуска программы.
    hasher: RandomState,
}

impl SecretClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Кладёт секрет в буфер обмена. Возвращает номер копирования для [`Self::clear_if_ours`].
    ///
    /// `secret: String` (а не `&str`): функция забирает строку во владение,
    /// и она уничтожается в конце функции — копий не остаётся.
    pub fn copy_secret(&self, secret: String) -> AppResult<u64> {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        // `&mut *guard` — изменяемая ссылка на сами данные под замком. Через неё можно
        // одновременно занять разные поля (clipboard и hasher): компилятор видит, что
        // они не пересекаются.
        let inner = &mut *guard;
        let clipboard = open(&mut inner.clipboard)?;
        set_secret_text(clipboard, &secret).map_err(clipboard_error)?;
        inner.fingerprint = Some(inner.hasher.hash_one(&secret));
        inner.generation += 1;
        Ok(inner.generation)
    }

    /// Очищает буфер, если в нём всё ещё наш секрет.
    /// `generation`: `Some(n)` — только если после копирования №n ничего не копировали;
    /// `None` — в любом случае (при выходе из программы).
    /// Возвращает `true`, если буфер очищен.
    pub fn clear_if_ours(&self, generation: Option<u64>) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let inner = &mut *guard;
        if generation.is_some_and(|g| g != inner.generation) {
            return false; // было новое копирование — его таймер разберётся сам
        }
        // take(): забираем значение, оставляя None. Повторный вызов уже ничего не сделает.
        let Some(fingerprint) = inner.fingerprint.take() else {
            return false;
        };
        let Some(clipboard) = inner.clipboard.as_mut() else {
            return false;
        };
        let still_ours = clipboard
            .get_text()
            .is_ok_and(|text| inner.hasher.hash_one(&text) == fingerprint);
        if !still_ours {
            return false; // пользователь скопировал что-то своё — не трогаем
        }
        match clipboard.clear() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "не удалось очистить буфер обмена");
                false
            }
        }
    }
}

/// Открывает системный буфер при первом обращении.
/// `get_or_insert` здесь не подходит: создание может завершиться ошибкой.
fn open(slot: &mut Option<arboard::Clipboard>) -> AppResult<&mut arboard::Clipboard> {
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(clipboard_error)?);
    }
    // `as_mut()`: &mut Option<T> → Option<&mut T>. Выше мы гарантировали Some.
    slot.as_mut()
        .ok_or_else(|| AppError::Clipboard("буфер обмена недоступен".into()))
}

/// Текст ошибки arboard не содержит данных буфера, его можно показывать.
fn clipboard_error(e: arboard::Error) -> AppError {
    AppError::Clipboard(e.to_string())
}

// `#[cfg(...)]` — условная компиляция: в сборку попадёт только одна из трёх функций,
// подходящая для текущей ОС. Остальные компилятор даже не проверяет.

/// Windows: не сохранять в журнал буфера (Win+V) и не отправлять в облако.
#[cfg(windows)]
fn set_secret_text(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    use arboard::SetExtWindows;
    clipboard.set().exclude_from_monitoring().text(text)
}

/// Linux: подсказка менеджерам буфера (KDE Klipper и др.) не сохранять в историю.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
fn set_secret_text(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    use arboard::SetExtLinux;
    clipboard.set().exclude_from_history().text(text)
}

/// Остальные системы: обычное копирование.
#[cfg(not(any(
    windows,
    all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    )
)))]
fn set_secret_text(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    clipboard.set_text(text)
}
