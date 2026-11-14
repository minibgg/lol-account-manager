//! Показ капчи (hCaptcha) при входе в аккаунт.
//!
//! Капчу нельзя решить программой — её специально придумали против автоматов. Поэтому
//! её решает человек. Чтобы не трогать окно Riot Client, мы показываем капчу сами:
//! поднимаем крошечный веб-сервер на `127.0.0.1` (виден только на этом компьютере),
//! открываем его страницу в браузере пользователя, человек проходит проверку, а страница
//! присылает нам «токен» решённой капчи обратно. Этот токен уходит в запрос входа.
//!
//! Сервер временный: живёт только пока ждём капчу, отвечает одной страницей и принимает
//! один токен. Ключ сайта и rqdata приходят от Riot Client (см. [`crate::riot_login`]).

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::error::{AppError, AppResult};
use crate::riot_login::Captcha;

/// Больше этого тело запроса не читаем (токен hCaptcha — несколько килобайт).
const MAX_BODY: usize = 64 * 1024;
/// Сколько всего ждём решения капчи.
const SOLVE_TIMEOUT: Duration = Duration::from_secs(180);

/// Экранирует текст для подстановки в значение HTML-атрибута (в кавычках).
fn attr_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// HTML-страница с капчей. Когда человек её решит, страница присылает токен на `/token`.
pub fn page_html(challenge: &Captcha) -> String {
    let sitekey = attr_escape(&challenge.sitekey);
    let rqdata = attr_escape(&challenge.rqdata);
    format!(
        r##"<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Вход — LoL Account Manager</title>
<script src="https://js.hcaptcha.com/1/api.js" async defer></script>
<style>
  :root {{ color-scheme: dark; }}
  body {{ margin:0; min-height:100vh; display:flex; flex-direction:column; align-items:center; justify-content:center;
         gap:16px; background:#0c0d10; color:#d6dae0;
         font-family:"Segoe UI",system-ui,sans-serif; text-align:center; padding:24px; }}
  h1 {{ font-size:18px; font-weight:600; margin:0; color:#f0c36a; }}
  p {{ margin:0; max-width:420px; line-height:1.5; color:#9aa3af; }}
  #status {{ min-height:20px; color:#7bd88f; }}
</style>
</head>
<body>
  <h1>LoL Account Manager</h1>
  <p>Пройдите проверку, чтобы войти в аккаунт. После этого вкладку можно закрыть — приложение продолжит само.</p>
  <div class="h-captcha" data-sitekey="{sitekey}" data-rqdata="{rqdata}" data-theme="dark"
       data-callback="onSolved" data-expired-callback="onExpired" data-error-callback="onError"></div>
  <p id="status"></p>
  <script>
    function say(text) {{ document.getElementById('status').textContent = text; }}
    function onSolved(token) {{
      say('Готово — возвращаюсь в приложение…');
      fetch('/token', {{ method: 'POST', body: token }})
        .then(function () {{ say('Проверка пройдена. Эту вкладку можно закрыть.'); }})
        .catch(function () {{ say('Не удалось передать результат в приложение.'); }});
    }}
    function onExpired() {{ say('Проверка истекла — пройдите её заново.'); }}
    function onError() {{ say('Ошибка проверки — обновите страницу и попробуйте снова.'); }}
  </script>
</body>
</html>
"##
    )
}

/// Ответ HTTP/1.1 с телом (текст).
fn http_response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Читает один HTTP-запрос: метод, путь и тело (если есть Content-Length).
async fn read_request(stream: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Читаем, пока не увидим конец заголовков (\r\n\r\n).
    let headers_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > MAX_BODY {
            return None;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..headers_end]);
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    // Длину тела берём из Content-Length (регистр заголовка не важен).
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
        .min(MAX_BODY);
    let mut body = buf[headers_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Some((method, path, body))
}

/// Показывает капчу и ждёт её решения. `on_ready` получает адрес страницы (её открывает
/// вызывающий — обычно в браузере пользователя). Возвращает токен решённой капчи.
pub async fn solve(challenge: &Captcha, on_ready: impl FnOnce(String)) -> AppResult<String> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .map_err(|e| AppError::AutoLogin(format!("не удалось открыть страницу капчи: {e}")))?;
    let port = listener.local_addr().map_err(|e| AppError::AutoLogin(e.to_string()))?.port();
    let page = page_html(challenge);
    on_ready(format!("http://127.0.0.1:{port}/"));

    let accept = async {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
            let Some((method, path, body)) = read_request(&mut stream).await else {
                continue;
            };
            let path = path.split('?').next().unwrap_or("/");
            if method == "POST" && path == "/token" {
                let token = String::from_utf8_lossy(&body).trim().to_string();
                let _ = stream.write_all(&http_response("200 OK", "text/plain", "ok")).await;
                let _ = stream.shutdown().await;
                if !token.is_empty() {
                    return token;
                }
            } else if method == "GET" && (path == "/" || path == "/index.html") {
                let _ = stream.write_all(&http_response("200 OK", "text/html", &page)).await;
                let _ = stream.shutdown().await;
            } else {
                let _ = stream.write_all(&http_response("404 Not Found", "text/plain", "not found")).await;
                let _ = stream.shutdown().await;
            }
        }
    };

    match tokio::time::timeout(SOLVE_TIMEOUT, accept).await {
        Ok(token) => Ok(token),
        Err(_) => Err(AppError::AutoLogin(
            "капча не была пройдена вовремя — нажмите Shift+Enter ещё раз".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_contains_sitekey_and_escapes_attributes() {
        let page = page_html(&Captcha { sitekey: "KEY-123".into(), rqdata: "rq\"&<>".into() });
        assert!(page.contains(r#"data-sitekey="KEY-123""#));
        assert!(page.contains("rq&quot;&amp;&lt;&gt;"), "кавычки и скобки экранированы");
        assert!(page.contains("js.hcaptcha.com/1/api.js"));
        assert!(page.contains("/token"));
    }

    /// Сервер отдаёт страницу по GET / и принимает токен по POST /token.
    #[tokio::test]
    async fn serves_page_and_receives_token() {
        let challenge = Captcha { sitekey: "KEY".into(), rqdata: "RQ".into() };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let solver = tokio::spawn(async move {
            solve(&challenge, move |url| {
                let _ = tx.send(url);
            })
            .await
        });
        let url = rx.await.unwrap();
        let addr = url.trim_start_matches("http://").trim_end_matches('/');

        // GET / — приходит HTML с капчей.
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut response = Vec::new();
        s.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("h-captcha"));

        // POST /token — отдаём токен, solve() его возвращает.
        let mut s = TcpStream::connect(addr).await.unwrap();
        let token = "solved-token-123";
        let request = format!("POST /token HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{token}", token.len());
        s.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        s.read_to_end(&mut response).await.unwrap();

        assert_eq!(solver.await.unwrap().unwrap(), token);
    }
}
