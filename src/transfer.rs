//! Экспорт и импорт списка аккаунтов — файл JSON, например чтобы перенести список
//! на другой компьютер или сохранить резервную копию.
//!
//! В файл попадает только несекретное: Riot ID, регион, теги и заметка.
//! **Логины и пароли не экспортируются никогда** — они хранятся только в системном
//! хранилище паролей; после импорта их вводят заново в форме «Изменить».
//! Не попадают и служебные поля (PUUID, имя записи в хранилище, кэш рангов):
//! после импорта данные загрузятся заново.
//!
//! Здесь только чистые функции (без диска и сети) — их легко проверить тестами.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::riot::{Region, RiotId};
use crate::storage::{Account, AccountInput};

/// Метка формата: по ней узнаём «свой» файл.
const FORMAT: &str = "lol-account-manager/accounts";
/// Версия формата. Новые поля добавляем с `#[serde(default)]`, тогда старые файлы
/// читаются и новой версией приложения. Номер поднимаем, только если старое
/// приложение не сможет правильно понять новый файл.
const VERSION: u32 = 1;

/// Больше этого файл импорта не читаем (тысячи аккаунтов — это ~1 МБ).
pub const MAX_IMPORT_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct ExportFile {
    format: String,
    version: u32,
    /// Когда сохранён (unix-время, секунды) — просто для справки.
    #[serde(default)]
    exported_at: i64,
    accounts: Vec<ExportedAccount>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExportedAccount {
    /// «Имя#TAG».
    riot_id: String,
    /// «EUW», «RU» — как в приложении.
    region: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    note: String,
}

/// Что получилось прочитать из файла импорта.
#[derive(Debug)]
pub struct ImportPlan {
    pub accounts: Vec<AccountInput>,
    /// Записи с ошибкой (неверный Riot ID или регион) — пропущены.
    pub invalid: usize,
}

/// Список аккаунтов → текст файла (JSON с отступами — его можно прочитать глазами).
pub fn export_json(accounts: &[Account], exported_at: i64) -> String {
    let file = ExportFile {
        format: FORMAT.to_string(),
        version: VERSION,
        exported_at,
        accounts: accounts
            .iter()
            .map(|a| ExportedAccount {
                riot_id: a.riot_id.to_string(),
                region: a.region.label().to_string(),
                tags: a.tags.clone(),
                note: a.note.clone(),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&file).expect("структура из строк всегда записывается в JSON")
}

/// Текст файла → аккаунты для добавления. Ошибка — если это вообще не наш файл.
pub fn parse_import(bytes: &[u8]) -> AppResult<ImportPlan> {
    let file: ExportFile = serde_json::from_slice(bytes).map_err(|_| AppError::NotAnExportFile)?;
    if file.format != FORMAT {
        return Err(AppError::NotAnExportFile);
    }
    if file.version > VERSION {
        return Err(AppError::ExportTooNew(file.version));
    }
    let mut accounts = Vec::new();
    let mut invalid = 0;
    for entry in file.accounts {
        // `.ok()` — Result → Option: ошибку разбора одной записи не считаем ошибкой файла.
        let riot_id = RiotId::parse(&entry.riot_id).ok();
        let region = entry.region.parse::<Region>().ok();
        match (riot_id, region) {
            (Some(riot_id), Some(region)) => accounts.push(AccountInput {
                riot_id,
                region,
                note: entry.note.trim().to_string(),
                tags: entry.tags,
            }),
            _ => invalid += 1,
        }
    }
    Ok(ImportPlan { accounts, invalid })
}

/// Имя файла по умолчанию: «lol-accounts-2026-11-07.json» (дата по UTC).
pub fn default_file_name(unix_secs: i64) -> String {
    let (year, month, day) = civil_date(unix_secs.div_euclid(86_400));
    format!("lol-accounts-{year:04}-{month:02}-{day:02}.json")
}

/// Дата (год, месяц, день) по номеру дня от 1970-01-01. Известный алгоритм
/// Howard Hinnant («civil_from_days»): годы считаются блоками по 400 лет (146 097 дней),
/// а год условно начинается 1 марта — тогда 29 февраля оказывается в конце года.
fn civil_date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468; // дней от 0000-03-01
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = if month_from_march < 10 { month_from_march + 3 } else { month_from_march - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, region: Region, tags: &[&str], note: &str) -> Account {
        Account {
            id: 1,
            riot_id: RiotId::parse(id).unwrap(),
            region,
            puuid: Some("api-puuid".into()),
            lcu_puuid: Some("lcu-puuid".into()),
            note: note.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            secret_ref: "6f1c2d3e-0000-4000-8000-000000000000".into(),
            has_login: true,
            has_password: true,
            created_at: 0,
            cache: None,
            history: vec![],
            archived_games: 0,
        }
    }

    #[test]
    fn export_roundtrip_without_secrets() {
        let accounts = [
            account("Шрек#RU1", Region::Ru, &["main"], "Основной"),
            account("Big Boss#EUW", Region::Euw, &[], ""),
        ];
        let json = export_json(&accounts, 1_794_009_600);
        // В файле нет ничего служебного или секретного.
        for forbidden in ["secret_ref", "6f1c2d3e", "puuid", "login", "password"] {
            assert!(!json.contains(forbidden), "в экспорте не должно быть «{forbidden}»:\n{json}");
        }

        let plan = parse_import(json.as_bytes()).unwrap();
        assert_eq!(plan.invalid, 0);
        let names: Vec<String> = plan.accounts.iter().map(|a| format!("{} {}", a.riot_id, a.region)).collect();
        assert_eq!(names, ["Шрек#RU1 RU", "Big Boss#EUW EUW"]);
        assert_eq!(plan.accounts[0].tags, ["main"]);
        assert_eq!(plan.accounts[0].note, "Основной");
    }

    #[test]
    fn import_skips_bad_entries_and_rejects_foreign_files() {
        let json = r#"{"format": "lol-account-manager/accounts", "version": 1, "accounts": [
            {"riot_id": "Good#EUW", "region": "euw1"},
            {"riot_id": "без решётки", "region": "EUW"},
            {"riot_id": "Name#TAG", "region": "Марс"}
        ]}"#;
        let plan = parse_import(json.as_bytes()).unwrap();
        assert_eq!(plan.accounts.len(), 1);
        assert_eq!(plan.accounts[0].region, Region::Euw, "регион и по платформе (euw1)");
        assert_eq!(plan.invalid, 2);

        assert_eq!(parse_import(b"not json").unwrap_err(), AppError::NotAnExportFile);
        let other = r#"{"format": "something-else", "version": 1, "accounts": []}"#;
        assert_eq!(parse_import(other.as_bytes()).unwrap_err(), AppError::NotAnExportFile);
        let newer = r#"{"format": "lol-account-manager/accounts", "version": 7, "accounts": []}"#;
        assert_eq!(parse_import(newer.as_bytes()).unwrap_err(), AppError::ExportTooNew(7));
    }

    #[test]
    fn file_name_has_date() {
        assert_eq!(default_file_name(0), "lol-accounts-1970-01-01.json");
        assert_eq!(default_file_name(1_794_009_600 + 3600), "lol-accounts-2026-11-07.json");
        assert_eq!(default_file_name(1_709_164_800), "lol-accounts-2024-02-29.json");
        assert_eq!(default_file_name(1_709_164_800 - 1), "lol-accounts-2024-02-28.json");
        assert_eq!(default_file_name(946_684_799), "lol-accounts-1999-12-31.json");
    }
}
