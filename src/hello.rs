//! Проверка «это точно вы?» через Windows Hello: отпечаток пальца, лицо или PIN-код.
//!
//! Нужна для настройки «спрашивать перед копированием пароля»: если за компьютером
//! оказался кто-то другой, скопировать пароль из открытой программы он не сможет.
//! Проверку целиком делает Windows (системное окно «Безопасность Windows»); программа
//! узнаёт только итог — подтверждено или нет. Ни PIN, ни отпечаток до нас не доходят.
//!
//! Системное окно нужно «привязать» к окну программы, поэтому используется не обычный
//! вызов WinRT, а его вариант для настольных программ (`IUserConsentVerifierInterop`).

use crate::error::AppResult;

/// Есть ли на этой системе Windows Hello (у нас — только Windows).
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// Показывает окно Windows Hello с текстом `message` и ждёт ответа. Блокирует поток —
/// вызывать из блокирующей задачи, не из потока окна. `Ok(())` — человек подтвердил.
pub fn verify(message: &str) -> AppResult<()> {
    imp::verify(message)
}

/// Настроен ли Windows Hello на этом компьютере (окно не показывается).
pub fn is_available() -> AppResult<bool> {
    imp::is_available()
}

#[cfg(windows)]
mod imp {
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult as Outcome, UserConsentVerifier, UserConsentVerifierAvailability as Availability,
    };
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::core::HSTRING;
    use windows_future::IAsyncOperation;

    use crate::error::{AppError, AppResult};

    const SETUP_HINT: &str = "Windows Hello не настроен: Параметры → Учётные записи → Варианты входа (нужен хотя бы PIN-код)";

    fn failed(e: windows::core::Error) -> AppError {
        AppError::HelloDenied(format!("Windows Hello недоступен: {e}"))
    }

    pub fn is_available() -> AppResult<bool> {
        let availability = UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|operation| operation.join())
            .map_err(failed)?;
        Ok(availability == Availability::Available)
    }

    pub fn verify(message: &str) -> AppResult<()> {
        // Сначала — есть ли вообще чем подтверждать.
        if !is_available()? {
            return Err(AppError::HelloDenied(SETUP_HINT.into()));
        }
        let interop: IUserConsentVerifierInterop =
            windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>().map_err(failed)?;
        // Окно, к которому Windows привяжет свой запрос: то, что сейчас перед глазами, —
        // пользователь только что нажал клавишу в нашей программе.
        // SAFETY: обычный вызов Windows API. GetForegroundWindow ничего не принимает,
        // а запросу проверки мы передаём действительные дескриптор окна и строку,
        // которые живут до конца вызова.
        let operation: IAsyncOperation<Outcome> = unsafe {
            let window = GetForegroundWindow();
            interop.RequestVerificationForWindowAsync(window, &HSTRING::from(message))
        }
        .map_err(failed)?;
        // join() ждёт, пока человек ответит в системном окне.
        match operation.join().map_err(failed)? {
            Outcome::Verified => Ok(()),
            Outcome::Canceled => Err(AppError::HelloDenied("проверка Windows Hello отменена".into())),
            Outcome::RetriesExhausted => Err(AppError::HelloDenied("слишком много неудачных попыток Windows Hello".into())),
            Outcome::DeviceBusy => Err(AppError::HelloDenied("Windows Hello сейчас занят — попробуйте ещё раз".into())),
            _ => Err(AppError::HelloDenied(SETUP_HINT.into())),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::error::{AppError, AppResult};

    pub fn verify(_message: &str) -> AppResult<()> {
        Err(AppError::HelloDenied("Windows Hello есть только в Windows".into()))
    }

    pub fn is_available() -> AppResult<bool> {
        Ok(false)
    }
}
