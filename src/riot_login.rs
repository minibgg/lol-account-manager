//! Вход в аккаунт через локальный API Riot Client — без нажатий клавиш за пользователя.
//!
//! Riot Client держит у себя на `127.0.0.1` маленький сервер, через который он сам входит
//! в аккаунты. Мы обращаемся к тому же серверу напрямую: отправляем логин и пароль
//! запросом, а не печатаем их в окно.
//!
//! Единственное, что по-прежнему делает человек, — проходит капчу (hCaptcha): Riot
//! требует её специально против автоматического входа, и решить её может только человек.
//! Токен капчи мы получаем из страницы, которую показываем в браузере (см. [`crate::captcha`]),
//! и передаём в запрос входа.
//!
//! Здесь — только разбор ответов Riot Client (чистые функции, без сети): их легко проверить
//! тестами. Сами запросы делает [`crate::lcu::LcuClient`].

use serde::Deserialize;

/// Что Riot Client ответил на шаг входа — и что делать дальше.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStep {
    /// Нужно решить капчу: её ключ и данные для hCaptcha.
    Captcha(Captcha),
    /// Капча не нужна — можно сразу отправлять логин и пароль.
    Ready,
    /// Нужен код подтверждения (двухфакторная защита): куда он отправлен и его длина.
    Multifactor { email: String, code_length: u8 },
    /// Вход выполнен.
    Success,
    /// Вход не удался — текст для пользователя.
    Failed(String),
}

/// Задача hCaptcha: ключ сайта и «привязанные данные» (rqdata) из ответа Riot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captcha {
    pub sitekey: String,
    pub rqdata: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AuthResponse {
    r#type: String,
    error: String,
    captcha: CaptchaDto,
    multifactor: MultifactorDto,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CaptchaDto {
    r#type: String,
    hcaptcha: HCaptchaDto,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HCaptchaDto {
    key: String,
    data: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MultifactorDto {
    email: String,
    #[serde(rename = "multiFactorCodeLength")]
    code_length: Option<u8>,
}

/// Понятный текст по коду ошибки Riot.
fn error_message(code: &str) -> String {
    match code {
        "auth_failure" | "invalid_credentials" => "неверный логин или пароль".into(),
        "rate_limited" => "слишком много попыток входа — подождите немного и попробуйте снова".into(),
        "captcha_not_allowed" | "invalid_captcha" | "captcha_failed" => {
            "капча не принята — попробуйте ещё раз".into()
        }
        "account_banned" | "suspended" => "аккаунт заблокирован Riot".into(),
        "" => "Riot Client отклонил вход".into(),
        other => format!("Riot Client вернул ошибку «{other}»"),
    }
}

/// Разбирает ответ Riot Client (тело JSON) в следующий шаг входа.
pub fn parse(json: &[u8]) -> AuthStep {
    let response: AuthResponse = match serde_json::from_slice(json) {
        Ok(response) => response,
        Err(_) => return AuthStep::Failed("не удалось разобрать ответ Riot Client".into()),
    };
    // Порядок важен: сначала завершённые состояния, потом «что нужно ещё».
    match response.r#type.as_str() {
        "success" => AuthStep::Success,
        "multifactor" => AuthStep::Multifactor {
            email: response.multifactor.email,
            // Код обычно шестизначный; длину берём из ответа, если она разумная.
            code_length: response.multifactor.code_length.filter(|n| (4..=8).contains(n)).unwrap_or(6),
        },
        "error" => AuthStep::Failed(error_message(&response.error)),
        _ => {
            // Любой другой тип («auth» и т. п.): нужна капча или нет.
            if response.captcha.r#type == "hcaptcha" && !response.captcha.hcaptcha.key.is_empty() {
                AuthStep::Captcha(Captcha {
                    sitekey: response.captcha.hcaptcha.key,
                    rqdata: response.captcha.hcaptcha.data,
                })
            } else if response.r#type == "auth" {
                AuthStep::Ready
            } else if !response.error.is_empty() {
                AuthStep::Failed(error_message(&response.error))
            } else {
                AuthStep::Failed(format!("Riot Client ответил «{}» — этот способ входа его не поддерживает", response.r#type))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captcha_step_from_start_response() {
        let json = br#"{"type":"auth","captcha":{"type":"hcaptcha","hcaptcha":{"key":"SITE-KEY","data":"rq-data"}}}"#;
        assert_eq!(
            parse(json),
            AuthStep::Captcha(Captcha { sitekey: "SITE-KEY".into(), rqdata: "rq-data".into() })
        );
    }

    #[test]
    fn no_captcha_is_ready() {
        let json = br#"{"type":"auth","captcha":{"type":"none","hcaptcha":{"key":"","data":""}}}"#;
        assert_eq!(parse(json), AuthStep::Ready);
    }

    #[test]
    fn success_multifactor_and_errors() {
        assert_eq!(parse(br#"{"type":"success"}"#), AuthStep::Success);
        assert_eq!(
            parse(br#"{"type":"multifactor","multifactor":{"email":"a@b.c","multiFactorCodeLength":6}}"#),
            AuthStep::Multifactor { email: "a@b.c".into(), code_length: 6 }
        );
        assert_eq!(parse(br#"{"type":"error","error":"auth_failure"}"#), AuthStep::Failed("неверный логин или пароль".into()));
        assert!(parse(br#"{"type":"error","error":"rate_limited"}"#).failed_text().unwrap().contains("попыток"));
        // Нечитаемый ответ — не паника.
        assert!(matches!(parse(b"not json"), AuthStep::Failed(_)));
    }

    impl AuthStep {
        fn failed_text(&self) -> Option<&str> {
            match self {
                AuthStep::Failed(text) => Some(text),
                _ => None,
            }
        }
    }
}
