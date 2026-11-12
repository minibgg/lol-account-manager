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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        assert_eq!(relative_to(1000, 1030), "только что");
        assert_eq!(relative_to(1000, 1000 + 5 * 60), "5 мин назад");
        assert_eq!(relative_to(1000, 1000 + 3 * 3600), "3 ч назад");
        assert_eq!(relative_to(1000, 1000 + 2 * 86_400), "2 дн назад");
        assert_eq!(relative_to(2000, 1000), "только что");
    }
}
