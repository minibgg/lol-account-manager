//! Одна копия программы на пользователя.
//!
//! Когда окно спрятано в трей, легко запустить программу второй раз с ярлыка. Вторая
//! копия была бы лишней (второй значок в трее, та же база). Поэтому при запуске:
//!
//! - первая копия занимает порт на 127.0.0.1 (адрес «этот же компьютер», снаружи
//!   недоступен) и слушает его;
//! - вторая видит, что порт занят, говорит первой «покажи окно» и сразу завершается.
//!
//! По сети ничего не уходит, а в сообщении нет никаких данных — только просьба показать окно.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use tracing::{debug, warn};

/// Порт по умолчанию (случайное число из «свободного» диапазона).
pub const DEFAULT_PORT: u16 = 47_823;

const HELLO: &[u8] = b"lol-account-manager:show\n";
const REPLY: &[u8] = b"lol-account-manager:ok\n";
const TIMEOUT: Duration = Duration::from_millis(700);

/// Чем закончилась попытка стать единственной копией.
pub enum Instance {
    /// Мы первые. Внутри — занятый порт: его надо отдать в [`listen`].
    First(TcpListener),
    /// Программа уже запущена, и мы попросили её показать окно — можно завершаться.
    AlreadyRunning,
    /// Порт занят кем-то другим (не нашей программой) — работаем как обычно, без защиты.
    Unknown,
}

pub fn acquire(port: u16) -> Instance {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpListener::bind(addr) {
        Ok(listener) => Instance::First(listener),
        Err(_) if ask_to_show(addr) => Instance::AlreadyRunning,
        Err(e) => {
            warn!(error = %e, port, "порт занят другой программой — защита от второй копии выключена");
            Instance::Unknown
        }
    }
}

/// Просит уже запущенную копию показать окно. true — она ответила.
fn ask_to_show(addr: SocketAddr) -> bool {
    let talk = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.write_all(HELLO)?;
        let mut reply = [0u8; 64];
        let n = stream.read(&mut reply)?;
        Ok(&reply[..n] == REPLY)
    };
    talk().unwrap_or(false)
}

/// Слушает порт в отдельном потоке и вызывает `on_show`, когда вторая копия просит
/// показать окно. Поток живёт до конца программы.
pub fn listen(listener: TcpListener, on_show: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new().name("single-instance".into()).spawn(move || {
        // incoming() — бесконечный итератор входящих подключений.
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(TIMEOUT));
            let _ = stream.set_write_timeout(Some(TIMEOUT));
            let mut hello = [0u8; 64];
            match stream.read(&mut hello) {
                // Отвечаем только на нашу фразу: в порт может постучаться что угодно.
                Ok(n) if &hello[..n] == HELLO => {
                    let _ = stream.write_all(REPLY);
                    debug!("вторая копия попросила показать окно");
                    on_show();
                }
                _ => {}
            }
        }
    });
    if let Err(e) = spawned {
        warn!(error = %e, "не удалось запустить поток защиты от второй копии");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// Свободный порт: просим у системы любой (порт 0) и тут же отпускаем.
    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn second_copy_asks_the_first_to_show() {
        let port = free_port();
        let Instance::First(listener) = acquire(port) else {
            panic!("первая копия должна занять порт");
        };
        let (tx, rx) = mpsc::channel();
        listen(listener, move || tx.send(()).unwrap());

        assert!(matches!(acquire(port), Instance::AlreadyRunning));
        rx.recv_timeout(Duration::from_secs(3)).expect("первая копия получила просьбу");
    }

    #[test]
    fn foreign_program_on_the_port_is_not_us() {
        // Порт занят, но на нашу фразу никто не отвечает.
        let foreign = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = foreign.local_addr().unwrap().port();
        assert!(matches!(acquire(port), Instance::Unknown));
    }

    #[test]
    fn garbage_does_not_show_the_window() {
        let port = free_port();
        let Instance::First(listener) = acquire(port) else {
            panic!("порт свободен");
        };
        let (tx, rx) = mpsc::channel();
        listen(listener, move || tx.send(()).unwrap());

        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
    }
}
