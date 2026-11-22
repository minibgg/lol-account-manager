//! Форматирование значений для интерфейса.

use crate::storage::unix_now;

/// «только что», «5 мин назад», «3 ч назад», «2 дн назад».
pub fn relative_time(unix_secs: i64) -> String {
    relative_to(unix_secs, unix_now())
}

fn relative_to(unix_secs: i64, now: i64) -> String {
    // max(0): если часы на компьютере перевели назад, не показываем «-5 мин».
    let secs = (now - unix_secs).max(0);
    match secs {
        0..=59 => "только что".to_string(),
        60..=3599 => format!("{} мин назад", secs / 60),
        3600..=86_399 => format!("{} ч назад", secs / 3600),
        _ => format!("{} дн назад", secs / 86_400),
    }
}

/// «через 12 мин», «через 5 ч», «через 2 дн.»; уже наступило — «сейчас».
pub fn until(unix_secs: i64, now: i64) -> String {
    match unix_secs - now {
        ..=59 => "сейчас".to_string(),
        secs @ 60..=3599 => format!("через {} мин", secs / 60),
        secs @ 3600..=86_399 => format!("через {} ч", secs / 3600),
        secs => format!("через {} дн.", secs / 86_400),
    }
}

/// «сб 29 нояб, 18:00» — по местному времени компьютера.
pub fn local_datetime(unix_secs: i64) -> String {
    wall_clock(unix_secs + local_offset_secs())
}

const WEEKDAYS: [&str; 7] = ["пн", "вт", "ср", "чт", "пт", "сб", "вс"];
const MONTHS: [&str; 12] = ["янв", "февр", "мар", "апр", "мая", "июн", "июл", "авг", "сент", "окт", "нояб", "дек"];

/// Секунды «по стенным часам» (unix-время уже со сдвигом часового пояса) →
/// «сб 29 нояб, 18:00». Дата из числа дней — алгоритм Говарда Хиннанта
/// (days_from_civil наоборот), без сторонних библиотек.
fn wall_clock(local: i64) -> String {
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // 1 января 1970 — четверг (в WEEKDAYS это индекс 3).
    let weekday = WEEKDAYS[(days + 3).rem_euclid(7) as usize];
    let z = days + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    format!(
        "{weekday} {day} {}, {:02}:{:02}",
        MONTHS[(month - 1) as usize],
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Сдвиг местного времени от UTC в секундах — из настроек часового пояса Windows
/// (с учётом летнего времени, если оно есть).
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut info = TIME_ZONE_INFORMATION::default();
    // SAFETY: функция только заполняет переданную ей структуру.
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    // Bias — сколько минут прибавить к местному времени, чтобы получить UTC.
    let bias = match id {
        1 => info.Bias + info.StandardBias,
        2 => info.Bias + info.DaylightBias,
        _ => info.Bias,
    };
    -i64::from(bias) * 60
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_dates() {
        assert_eq!(wall_clock(0), "чт 1 янв, 00:00");
        // 2026-11-28 18:00 — суббота.
        assert_eq!(wall_clock(1_795_888_800), "сб 28 нояб, 18:00");
        // 2024-02-29 — високосный год.
        assert_eq!(wall_clock(1_709_164_800 + 9 * 3600 + 5 * 60), "чт 29 февр, 09:05");
    }

    #[test]
    fn until_times() {
        assert_eq!(until(100, 100), "сейчас");
        assert_eq!(until(100 + 12 * 60, 100), "через 12 мин");
        assert_eq!(until(100 + 5 * 3600, 100), "через 5 ч");
        assert_eq!(until(100 + 2 * 86_400 + 5, 100), "через 2 дн.");
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_to(1000, 1030), "только что");
        assert_eq!(relative_to(1000, 1000 + 5 * 60), "5 мин назад");
        assert_eq!(relative_to(1000, 1000 + 3 * 3600), "3 ч назад");
        assert_eq!(relative_to(1000, 1000 + 2 * 86_400), "2 дн назад");
        assert_eq!(relative_to(2000, 1000), "только что");
    }
}
