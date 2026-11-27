//! SQLite: аккаунты (БЕЗ секретов), кэш профилей и рангов, настройки.
//!
//! Логины и пароли сюда не попадают никогда — только флаги «есть логин / есть пароль»
//! и `secret_ref` — случайный UUID, по которому запись ищется в системном хранилище.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::history::{self, DAY, HISTORY_DAYS, Queue, RankPoint};
use crate::lobby::Meeting;
use crate::riot::{
    AccountSnapshot, DataSource, Division, MatchDetails, MatchSummary, RankedQueues, RankedStats, Region,
    RiotId, Tier,
};

pub type AccountId = i64;

/// Другой игрок в игре из архива (см. [`Db::archived_players`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedPlayer {
    /// `None` — имя скрыто или неизвестно.
    pub riot_id: Option<RiotId>,
    /// Название чемпиона (как его записал клиент или Riot API).
    pub champion: String,
    /// В команде аккаунта.
    pub ally: bool,
}

/// Миграции схемы. Номер версии = индекс + 1; текущая версия хранится в
/// `PRAGMA user_version`. Новые изменения схемы — только НОВЫМИ элементами в конце,
/// старые не трогаем: у пользователей они уже применены.
const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE accounts (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        game_name     TEXT    NOT NULL,
        tag_line      TEXT    NOT NULL,
        region        TEXT    NOT NULL,
        puuid         TEXT,
        note          TEXT    NOT NULL DEFAULT '',
        tags          TEXT    NOT NULL DEFAULT '[]',  -- JSON-массив строк
        secret_ref    TEXT    NOT NULL UNIQUE,        -- UUID записи в keyring (это НЕ секрет)
        has_login     INTEGER NOT NULL DEFAULT 0,
        has_password  INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL                -- unix-время, секунды
    );
    CREATE UNIQUE INDEX accounts_puuid ON accounts(puuid) WHERE puuid IS NOT NULL;

    CREATE TABLE account_cache (
        account_id      INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
        profile_icon_id INTEGER NOT NULL,
        summoner_level  INTEGER NOT NULL,
        ranked_json     TEXT    NOT NULL,
        fetched_at      INTEGER NOT NULL
    );

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
    // v2: PUUID из клиента League (LCU). Он отличается от PUUID Riot API,
    // поэтому хранится отдельно (см. riot::DataSource).
    r#"
    ALTER TABLE accounts ADD COLUMN lcu_puuid TEXT;
    CREATE UNIQUE INDEX accounts_lcu_puuid ON accounts(lcu_puuid) WHERE lcu_puuid IS NOT NULL;
    "#,
    // v3: последние игры (JSON-массив MatchSummary). NULL — ещё не загружались.
    r#"
    ALTER TABLE account_cache ADD COLUMN matches_json TEXT;
    "#,
    // v4: история рангов — строка на каждое изменение ранга в очереди (см. history.rs).
    r#"
    CREATE TABLE rank_history (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        queue      TEXT    NOT NULL,  -- 'solo' | 'flex'
        tier       TEXT    NOT NULL,  -- 'GOLD'
        division   TEXT    NOT NULL,  -- 'II'
        lp         INTEGER NOT NULL,
        wins       INTEGER NOT NULL,
        losses     INTEGER,           -- NULL — неизвестно (клиент League скрывает)
        taken_at   INTEGER NOT NULL   -- unix-время, секунды
    );
    CREATE INDEX rank_history_account ON rank_history(account_id, taken_at);
    "#,
    // v5: архив игр — каждая игра, которую приложение видело, без ограничения количества.
    // summary_json — MatchSummary (игра глазами аккаунта), details_json — MatchDetails
    // (обе команды; NULL — ещё не загружены). Игры из кэша последних переносит сюда
    // `archive_cached_matches` сразу после миграции.
    r#"
    CREATE TABLE matches (
        account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        match_id     TEXT    NOT NULL,
        played_at    INTEGER NOT NULL,
        summary_json TEXT    NOT NULL,
        details_json TEXT,
        PRIMARY KEY (account_id, match_id)
    );
    CREATE INDEX matches_played ON matches(account_id, played_at);
    "#,
];

/// За сколько дней загружаем время ранговых игр вместе со списком аккаунтов.
/// С запасом: самый большой запас дней до сгорания ранга — 30.
pub const RANKED_GAMES_DAYS: i64 = 180;

/// С этой версии схемы есть архив игр.
const ARCHIVE_VERSION: i64 = 5;

/// Точки истории за последние HISTORY_DAYS дней и по одной (последней) до этого — с какого
/// ранга начинался период. ?1 — граница по времени, ?2 — аккаунт (NULL — все).
/// Во втором SELECT у SQLite особое правило: при MAX() остальные столбцы берутся из той же
/// строки, где максимум.
const SELECT_HISTORY: &str = "
    SELECT account_id, queue, tier, division, lp, wins, losses, taken_at, id
    FROM rank_history
    WHERE taken_at >= ?1 AND (?2 IS NULL OR account_id = ?2)
    UNION ALL
    SELECT account_id, queue, tier, division, lp, wins, losses, MAX(taken_at) AS taken_at, id
    FROM rank_history
    WHERE taken_at < ?1 AND (?2 IS NULL OR account_id = ?2)
    GROUP BY account_id, queue
    ORDER BY taken_at, id";

const SELECT_ACCOUNT: &str = "
    SELECT a.id, a.game_name, a.tag_line, a.region, a.puuid, a.lcu_puuid, a.note, a.tags, a.secret_ref,
           a.has_login, a.has_password, a.created_at,
           c.profile_icon_id, c.summoner_level, c.ranked_json, c.matches_json, c.fetched_at,
           (SELECT COUNT(*) FROM matches m WHERE m.account_id = a.id) AS archived_games
    FROM accounts a
    LEFT JOIN account_cache c ON c.account_id = a.id";

/// Аккаунт, как он хранится у нас (без секретов).
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: AccountId,
    pub riot_id: RiotId,
    pub region: Region,
    /// PUUID Riot API. Появляется после первого обновления через Riot API.
    pub puuid: Option<String>,
    /// PUUID клиента League. Появляется после первого обновления через клиент.
    pub lcu_puuid: Option<String>,
    pub note: String,
    pub tags: Vec<String>,
    /// Имя записи в системном хранилище паролей. Не секрет — просто идентификатор.
    pub secret_ref: String,
    pub has_login: bool,
    pub has_password: bool,
    pub created_at: i64,
    /// Последние полученные данные — чтобы показывать их без сети.
    pub cache: Option<CachedProfile>,
    /// Как менялись ранги (обе очереди вперемешку, по порядку времени): последние
    /// [`HISTORY_DAYS`] дней и по одной точке до этого.
    pub history: Vec<RankPoint>,
    /// Сколько игр в архиве (сами игры — `Db::archived_matches`, по запросу).
    pub archived_games: usize,
    /// Когда игрались ранговые игры за последние [`RANKED_GAMES_DAYS`] дней (очередь
    /// и unix-время, от старых к новым) — по ним оценивается сгорание ранга (decay.rs).
    pub ranked_games: Vec<(Queue, i64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedProfile {
    pub profile_icon_id: i32,
    pub summoner_level: i64,
    pub ranked: RankedQueues,
    /// Последние игры, новые первыми. `None` — ещё не загружались, пустой список — игр нет.
    pub matches: Option<Vec<MatchSummary>>,
    /// Когда получены (unix-время, секунды).
    pub fetched_at: i64,
}

/// Поля формы «Добавить / Изменить аккаунт» (без логина и пароля).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountInput {
    pub riot_id: RiotId,
    pub region: Region,
    pub note: String,
    pub tags: Vec<String>,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Открывает (или создаёт) файл базы и применяет миграции.
    pub fn open(path: &Path) -> AppResult<Self> {
        Self::init(Connection::open(path)?)
    }

    /// База в памяти — для тестов.
    pub fn open_in_memory() -> AppResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> AppResult<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        // Внешние ключи в SQLite выключены по умолчанию — без этого не сработает
        // ON DELETE CASCADE (удаление кэша вместе с аккаунтом).
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn schema_version(&self) -> AppResult<i64> {
        Ok(self
            .conn
            .query_row("SELECT user_version FROM pragma_user_version", [], |r| r.get(0))?)
    }

    fn migrate(&mut self) -> AppResult<()> {
        let current = self.schema_version()?;
        let latest = MIGRATIONS.len() as i64;
        if current > latest {
            return Err(AppError::Database(format!(
                "база создана более новой версией приложения (схема v{current})"
            )));
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let version = index as i64 + 1;
            // Транзакция: либо миграция применится целиком, либо не применится вовсе.
            // Если до commit() случится ошибка, `tx` уничтожится (drop) и всё откатится само.
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {version};"))?;
            tx.commit()?;
            info!(version, "применена миграция базы данных");
        }
        // Архив только что появился — переносим в него игры, которые уже лежат в кэше.
        // (В Rust, а не в SQL: JSON разбирает serde, испорченные записи просто пропускаются.)
        if current < ARCHIVE_VERSION && latest >= ARCHIVE_VERSION {
            self.archive_cached_matches()?;
        }
        Ok(())
    }

    /// Игры из кэша последних (`account_cache.matches_json`) — в архив.
    fn archive_cached_matches(&mut self) -> AppResult<()> {
        let cached: Vec<(AccountId, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT account_id, matches_json FROM account_cache WHERE matches_json IS NOT NULL")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let tx = self.conn.transaction()?;
        for (account_id, json) in cached {
            let Ok(games) = serde_json::from_str::<Vec<MatchSummary>>(&json) else {
                continue; // испорченный кэш — пропускаем
            };
            for game in &games {
                archive_summary(&tx, account_id, game)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ----- Аккаунты ---------------------------------------------------------

    pub fn list_accounts(&self) -> AppResult<Vec<Account>> {
        let mut stmt = self.conn.prepare(&format!("{SELECT_ACCOUNT} ORDER BY a.id"))?;
        // query_and_then отдаёт итератор из AppResult<Account>.
        // collect() в AppResult<Vec<_>> соберёт все строки или вернёт первую ошибку.
        let mut accounts = stmt
            .query_and_then([], account_from_row)?
            .collect::<AppResult<Vec<_>>>()?;
        let mut histories = self.rank_history(None)?;
        let mut ranked_games = self.ranked_games(None)?;
        for account in &mut accounts {
            // remove — забрать из словаря (без копирования); нет истории — пустой список.
            account.history = histories.remove(&account.id).unwrap_or_default();
            account.ranked_games = ranked_games.remove(&account.id).unwrap_or_default();
        }
        Ok(accounts)
    }

    pub fn get_account(&self, id: AccountId) -> AppResult<Account> {
        let mut stmt = self.conn.prepare(&format!("{SELECT_ACCOUNT} WHERE a.id = ?1"))?;
        let mut rows = stmt.query_and_then([id], account_from_row)?;
        // next() даёт Option<AppResult<Account>> — «строка есть?» и «прочиталась ли она?».
        // ok_or: нет строки → AccountMissing. Два `?`: первый проверяет наличие строки,
        // второй — что строка прочиталась без ошибки.
        let mut account = rows.next().ok_or(AppError::AccountMissing)??;
        account.history = self.rank_history(Some(id))?.remove(&id).unwrap_or_default();
        account.ranked_games = self.ranked_games(Some(id))?.remove(&id).unwrap_or_default();
        Ok(account)
    }

    /// Время недавних ранговых игр из архива по аккаунтам (одного или всех — `None`).
    /// Очередь и признак ремейка лежат внутри JSON игры — их достаёт сам SQLite
    /// (`json_extract`). Ремейки не считаются: такая игра ранг не «продлевает».
    fn ranked_games(&self, account: Option<AccountId>) -> AppResult<HashMap<AccountId, Vec<(Queue, i64)>>> {
        let since = unix_now() - RANKED_GAMES_DAYS * DAY;
        let mut stmt = self.conn.prepare(
            "SELECT account_id, played_at, json_extract(summary_json, '$.queue_id') AS queue_id
             FROM matches
             WHERE played_at >= ?1 AND (?2 IS NULL OR account_id = ?2)
               AND json_extract(summary_json, '$.queue_id') IN (420, 440)
               AND COALESCE(json_extract(summary_json, '$.remake'), 0) = 0
             ORDER BY played_at",
        )?;
        let mut rows = stmt.query(params![since, account])?;
        let mut out: HashMap<AccountId, Vec<(Queue, i64)>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let account_id: AccountId = row.get("account_id")?;
            if let Some(queue) = Queue::from_queue_id(row.get("queue_id")?) {
                out.entry(account_id).or_default().push((queue, row.get("played_at")?));
            }
        }
        Ok(out)
    }

    /// История рангов по аккаунтам (одного или всех — `None`), см. [`SELECT_HISTORY`].
    fn rank_history(&self, account: Option<AccountId>) -> AppResult<HashMap<AccountId, Vec<RankPoint>>> {
        let since = unix_now() - HISTORY_DAYS * DAY;
        let mut stmt = self.conn.prepare(SELECT_HISTORY)?;
        let mut rows = stmt.query(params![since, account])?;
        let mut out: HashMap<AccountId, Vec<RankPoint>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let account_id: AccountId = row.get("account_id")?;
            match point_from_row(row)? {
                // entry().or_default() — список аккаунта (создать пустой, если его ещё нет).
                Some(point) => out.entry(account_id).or_default().push(point),
                None => warn!(account_id, "точка истории рангов не распознана — пропускаем"),
            }
        }
        Ok(out)
    }

    pub fn insert_account(&mut self, input: &AccountInput) -> AppResult<Account> {
        self.ensure_unique(&input.riot_id, input.region, None)?;
        let secret_ref = uuid::Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO accounts (game_name, tag_line, region, note, tags, secret_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                input.riot_id.game_name,
                input.riot_id.tag_line,
                input.region.label(),
                input.note.trim(),
                tags_json(&input.tags),
                secret_ref,
                unix_now(),
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_account(id)
    }

    pub fn update_account(&mut self, id: AccountId, input: &AccountInput) -> AppResult<Account> {
        let old = self.get_account(id)?;
        self.ensure_unique(&input.riot_id, input.region, Some(id))?;

        // Другой Riot ID или регион — по сути другой аккаунт: старые PUUID и кэш не годятся.
        // Смена только регистра букв («faker» → «Faker») — тот же аккаунт.
        let identity_changed =
            !same_riot_id(&old.riot_id, &input.riot_id) || old.region != input.region;

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE accounts SET game_name = ?1, tag_line = ?2, region = ?3, note = ?4, tags = ?5
             WHERE id = ?6",
            params![
                input.riot_id.game_name,
                input.riot_id.tag_line,
                input.region.label(),
                input.note.trim(),
                tags_json(&input.tags),
                id,
            ],
        )?;
        if identity_changed {
            tx.execute("UPDATE accounts SET puuid = NULL, lcu_puuid = NULL WHERE id = ?1", [id])?;
            tx.execute("DELETE FROM account_cache WHERE account_id = ?1", [id])?;
            tx.execute("DELETE FROM rank_history WHERE account_id = ?1", [id])?;
            tx.execute("DELETE FROM matches WHERE account_id = ?1", [id])?;
        }
        tx.commit()?;
        self.get_account(id)
    }

    pub fn delete_account(&mut self, id: AccountId) -> AppResult<()> {
        // Кэш удалится сам благодаря ON DELETE CASCADE.
        let deleted = self.conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        if deleted == 0 {
            return Err(AppError::AccountMissing);
        }
        // Инвентарь аккаунта хранится в настройках (см. backend::inventory).
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", [format!("inventory_{id}")])?;
        Ok(())
    }

    pub fn set_credential_flags(
        &mut self,
        id: AccountId,
        has_login: bool,
        has_password: bool,
    ) -> AppResult<()> {
        self.conn.execute(
            "UPDATE accounts SET has_login = ?1, has_password = ?2 WHERE id = ?3",
            params![has_login, has_password, id],
        )?;
        Ok(())
    }

    /// Сохраняет результат обновления: PUUID источника, актуальный Riot ID и кэш.
    pub fn save_snapshot(
        &mut self,
        id: AccountId,
        snapshot: &AccountSnapshot,
        fetched_at: i64,
    ) -> AppResult<()> {
        // У каждого источника свой PUUID — и свой столбец. Имя столбца подставляем
        // в SQL через format!: это безопасно, оно берётся только из этих двух констант,
        // а не из пользовательского ввода.
        let column = match snapshot.source {
            DataSource::RiotApi => "puuid",
            DataSource::Lcu => "lcu_puuid",
        };

        // Тот же Riot-аккаунт уже есть в списке под другой записью?
        let other: Option<AccountId> = self
            .conn
            .query_row(
                &format!("SELECT id FROM accounts WHERE {column} = ?1 AND id <> ?2"),
                params![snapshot.puuid, id],
                |r| r.get(0),
            )
            .optional()?;
        if other.is_some() {
            return Err(AppError::DuplicateAccount);
        }

        let ranked_json = to_json(&snapshot.ranked)?;
        // None — игры не загрузились: в UPSERT ниже COALESCE оставит прежние.
        // `transpose`: Option<Result<T>> → Result<Option<T>>, чтобы сработал `?`.
        let matches_json = snapshot.matches.as_ref().map(to_json).transpose()?;

        let tx = self.conn.transaction()?;
        let updated = tx.execute(
            &format!("UPDATE accounts SET {column} = ?1, game_name = ?2, tag_line = ?3 WHERE id = ?4"),
            params![
                snapshot.puuid,
                snapshot.riot_id.game_name,
                snapshot.riot_id.tag_line,
                id
            ],
        )?;
        if updated == 0 {
            return Err(AppError::AccountMissing); // tx откатится при выходе из функции
        }
        // UPSERT: вставить строку кэша или обновить существующую.
        // COALESCE(a, b) — первое не-NULL значение: новых игр нет → остаются старые.
        tx.execute(
            "INSERT INTO account_cache (account_id, profile_icon_id, summoner_level, ranked_json, matches_json, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_id) DO UPDATE SET
                 profile_icon_id = excluded.profile_icon_id,
                 summoner_level  = excluded.summoner_level,
                 ranked_json     = excluded.ranked_json,
                 matches_json    = COALESCE(excluded.matches_json, account_cache.matches_json),
                 fetched_at      = excluded.fetched_at",
            params![
                id,
                snapshot.profile_icon_id,
                snapshot.summoner_level,
                ranked_json,
                matches_json,
                fetched_at
            ],
        )?;
        // История: новая точка, только если ранг в очереди изменился с прошлой записи.
        for queue in Queue::ALL {
            let Some(stats) = queue.stats(&snapshot.ranked) else {
                continue; // без ранга (или сезон сбросился) — точку не пишем
            };
            let last = last_point(&tx, id, queue)?;
            if last.is_none_or(|last| history::has_progress(&last, stats)) {
                insert_point(&tx, id, queue, stats, fetched_at)?;
            }
        }
        // Архив: все пришедшие игры (без ограничения количества) и их подробности.
        for game in snapshot.matches.iter().flatten() {
            archive_summary(&tx, id, game)?;
        }
        for details in &snapshot.details {
            archive_details(&tx, id, details)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ----- Архив игр --------------------------------------------------------

    /// Все сохранённые игры аккаунта, новые первыми. `limit` — не больше стольких.
    /// Испорченные записи пропускаются.
    pub fn archived_matches(&self, id: AccountId, limit: Option<usize>) -> AppResult<Vec<MatchSummary>> {
        // LIMIT -1 в SQLite — «без ограничения».
        let limit = limit.map_or(-1, |l| l as i64);
        let mut stmt = self.conn.prepare(
            "SELECT summary_json FROM matches WHERE account_id = ?1
             ORDER BY played_at DESC, match_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![id, limit], |r| r.get::<_, String>(0))?;
        let mut games = Vec::new();
        for json in rows {
            match serde_json::from_str::<MatchSummary>(&json?) {
                Ok(game) => games.push(game),
                Err(e) => warn!(error = %e, "игра в архиве повреждена — пропускаем"),
            }
        }
        Ok(games)
    }

    /// Подробности сохранённой игры (обе команды). `None` — ещё не сохранены.
    pub fn archived_details(&self, id: AccountId, match_id: &str) -> AppResult<Option<MatchDetails>> {
        let json: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT details_json FROM matches WHERE account_id = ?1 AND match_id = ?2",
                params![id, match_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.flatten().and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// Сохранить подробности игры, которая уже есть в архиве.
    pub fn save_details(&mut self, id: AccountId, details: &MatchDetails) -> AppResult<()> {
        archive_details(&self.conn, id, details)
    }

    /// Игры архива без подробностей, новые первыми — их догружает фоновый поток.
    pub fn matches_without_details(&self, id: AccountId, limit: usize) -> AppResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT match_id FROM matches WHERE account_id = ?1 AND details_json IS NULL
             ORDER BY played_at DESC LIMIT ?2",
        )?;
        let ids = stmt
            .query_map(params![id, limit as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
    }

    /// Другие игроки в играх аккаунта (кроме него самого) — для «напарников» в окне
    /// «Профиль» и поиска по нику и чемпиону во «Всех играх»: номер игры → игроки.
    /// Только игры с подробностями. Свою команду узнаём по отметке `is_me`, а не по
    /// порядку команд: так надёжнее для старых записей архива.
    pub fn archived_players(&self, id: AccountId) -> AppResult<HashMap<String, Vec<ArchivedPlayer>>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.match_id,
                    json_extract(p.value, '$.riot_id.game_name'),
                    json_extract(p.value, '$.riot_id.tag_line'),
                    json_extract(p.value, '$.stats.champion'),
                    EXISTS (SELECT 1 FROM json_each(t.value, '$.players') q
                            WHERE json_extract(q.value, '$.is_me') = 1)
             FROM matches m,
                  json_each(m.details_json, '$.teams') t,
                  json_each(t.value, '$.players') p
             WHERE m.account_id = ?1
               AND m.details_json IS NOT NULL
               AND json_extract(p.value, '$.is_me') = 0",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, bool>(4)?,
            ))
        })?;
        let mut players: HashMap<String, Vec<ArchivedPlayer>> = HashMap::new();
        for row in rows {
            let (match_id, game_name, tag_line, champion, ally) = row?;
            players.entry(match_id).or_default().push(ArchivedPlayer {
                riot_id: game_name.map(|game_name| RiotId { game_name, tag_line }),
                champion,
                ally,
            });
        }
        Ok(players)
    }

    /// Прошлые игры с игроками `players` из архивов всех аккаунтов — для пометки «уже
    /// встречались» на вкладке «Лобби». Ищем по Riot ID: в подробностях игр он есть у всех
    /// десяти игроков, а PUUID — нет. Регистр не важен. Ремейки не берём.
    pub fn meetings(&self, players: &[RiotId]) -> AppResult<Vec<(RiotId, Meeting)>> {
        // Грубый отбор по тегу — в SQL (lower() в SQLite понимает только латиницу, поэтому
        // и здесь to_ascii_lowercase), точное сравнение имени и тега — уже здесь, в Rust.
        let mut tags: Vec<String> = players.iter().map(|p| p.tag_line.to_ascii_lowercase()).collect();
        tags.sort();
        tags.dedup();
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (1..=tags.len()).map(|i| format!("?{i}")).collect();
        // json_each разворачивает массив JSON в строки: каждая команда, потом каждый игрок.
        // Своя команда в архиве всегда первая (key = 0).
        let sql = format!(
            "SELECT m.match_id, m.played_at, a.game_name, a.tag_line, t.key,
                    json_extract(t.value, '$.win'),
                    json_extract(p.value, '$.riot_id.game_name'),
                    json_extract(p.value, '$.riot_id.tag_line'),
                    json_extract(p.value, '$.stats.champion')
             FROM matches m
             JOIN accounts a ON a.id = m.account_id,
                  json_each(m.details_json, '$.teams') t,
                  json_each(t.value, '$.players') p
             WHERE m.details_json IS NOT NULL
               AND json_extract(m.details_json, '$.remake') = 0
               AND json_extract(p.value, '$.is_me') = 0
               AND lower(json_extract(p.value, '$.riot_id.tag_line')) IN ({})",
            placeholders.join(", ")
        );
        let wanted: Vec<(String, &RiotId)> = players.iter().map(|p| (p.to_string().to_lowercase(), p)).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(tags.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                format!("{}#{}", r.get::<_, String>(2)?, r.get::<_, String>(3)?),
                r.get::<_, i64>(4)?,
                r.get::<_, Option<bool>>(5)?.unwrap_or(false),
                r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            ))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (match_id, played_at, account, team, team_win, name, tag, champion) = row?;
            let key = format!("{name}#{tag}").to_lowercase();
            let Some((_, player)) = wanted.iter().find(|(k, _)| *k == key) else {
                continue;
            };
            let same_team = team == 0;
            found.push((
                (*player).clone(),
                Meeting {
                    match_id,
                    played_at,
                    account,
                    same_team,
                    won: team_win == same_team,
                    champion,
                },
            ));
        }
        Ok(found)
    }

    // ----- Настройки (только несекретные!) ----------------------------------

    pub fn setting(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&mut self, key: &str) -> AppResult<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Все настройки, ключ которых начинается с `prefix` («inventory_» → инвентари всех
    /// аккаунтов). LIKE не подходит: «_» в нём — любой символ.
    pub fn settings_with_prefix(&self, prefix: &str) -> AppResult<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM settings WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key")?;
        let rows = stmt
            .query_map([prefix], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    // ----- Вспомогательное --------------------------------------------------

    /// Нет ли уже такого Riot ID в этом регионе (без учёта регистра).
    fn ensure_unique(
        &self,
        riot_id: &RiotId,
        region: Region,
        except: Option<AccountId>,
    ) -> AppResult<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, game_name, tag_line FROM accounts WHERE region = ?1")?;
        let rows = stmt.query_map([region.label()], |r| {
            Ok((r.get::<_, AccountId>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (id, game_name, tag_line) = row?;
            if Some(id) != except && same_riot_id(&RiotId { game_name, tag_line }, riot_id) {
                return Err(AppError::DuplicateAccount);
            }
        }
        Ok(())
    }
}

fn account_from_row(row: &Row<'_>) -> AppResult<Account> {
    let region: String = row.get("region")?;
    let tags: String = row.get("tags")?;

    // LEFT JOIN: если кэша нет, все его столбцы — NULL, поэтому читаем как Option.
    let cache = match row.get::<_, Option<i64>>("fetched_at")? {
        None => None,
        Some(fetched_at) => {
            let ranked_json: String = row.get("ranked_json")?;
            // Игры — не главное: испорченный JSON — как будто их ещё не загружали.
            let matches = row
                .get::<_, Option<String>>("matches_json")?
                .and_then(|json| match serde_json::from_str::<Vec<MatchSummary>>(&json) {
                    Ok(matches) => Some(matches),
                    Err(e) => {
                        warn!(error = %e, "кэш последних игр повреждён — игнорируем");
                        None
                    }
                });
            match serde_json::from_str::<RankedQueues>(&ranked_json) {
                Ok(ranked) => Some(CachedProfile {
                    profile_icon_id: row.get("profile_icon_id")?,
                    summoner_level: row.get("summoner_level")?,
                    ranked,
                    matches,
                    fetched_at,
                }),
                Err(e) => {
                    warn!(error = %e, "кэш рангов повреждён — игнорируем, обновится при следующем запросе");
                    None
                }
            }
        }
    };

    Ok(Account {
        id: row.get("id")?,
        riot_id: RiotId {
            game_name: row.get("game_name")?,
            tag_line: row.get("tag_line")?,
        },
        region: region.parse()?,
        puuid: row.get("puuid")?,
        lcu_puuid: row.get("lcu_puuid")?,
        note: row.get("note")?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        secret_ref: row.get("secret_ref")?,
        has_login: row.get("has_login")?,
        has_password: row.get("has_password")?,
        created_at: row.get("created_at")?,
        cache,
        history: Vec::new(), // заполняют list_accounts / get_account
        archived_games: row.get::<_, i64>("archived_games")? as usize,
        ranked_games: Vec::new(), // заполняют list_accounts / get_account
    })
}

/// Игра — в архив (или обновить запись, если показателей стало больше).
fn archive_summary(conn: &Connection, id: AccountId, game: &MatchSummary) -> AppResult<()> {
    conn.execute(
        "INSERT INTO matches (account_id, match_id, played_at, summary_json) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id, match_id) DO UPDATE SET
             played_at    = excluded.played_at,
             -- Позицию игрока иногда знаем только мы (запомнили во время игры), а в новой
             -- записи от клиента её нет: тогда переносим её из старой записи.
             summary_json = CASE
                 WHEN COALESCE(json_extract(excluded.summary_json, '$.position'), '') = ''
                  AND COALESCE(json_extract(matches.summary_json, '$.position'), '') <> ''
                 THEN json_set(excluded.summary_json, '$.position', json_extract(matches.summary_json, '$.position'))
                 ELSE excluded.summary_json
             END",
        params![id, game.id, game.played_at, to_json(game)?],
    )?;
    Ok(())
}

/// Подробности игры — к её записи в архиве (если записи нет, ничего не делаем).
fn archive_details(conn: &Connection, id: AccountId, details: &MatchDetails) -> AppResult<()> {
    conn.execute(
        "UPDATE matches SET details_json = ?3 WHERE account_id = ?1 AND match_id = ?2",
        params![id, details.id, to_json(details)?],
    )?;
    Ok(())
}

/// Последняя записанная точка истории очереди (только ранг).
fn last_point(conn: &Connection, id: AccountId, queue: Queue) -> AppResult<Option<RankedStats>> {
    let row = conn
        .query_row(
            "SELECT queue, tier, division, lp, wins, losses, taken_at FROM rank_history
             WHERE account_id = ?1 AND queue = ?2
             ORDER BY taken_at DESC, id DESC LIMIT 1",
            params![id, queue.key()],
            |row| Ok(point_from_row(row)),
        )
        .optional()?;
    // row: Option<AppResult<Option<RankPoint>>> → transpose → AppResult<Option<Option<..>>>.
    Ok(row.transpose()?.flatten().map(|point| point.stats))
}

fn insert_point(
    conn: &Connection,
    id: AccountId,
    queue: Queue,
    stats: &RankedStats,
    taken_at: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO rank_history (account_id, queue, tier, division, lp, wins, losses, taken_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            queue.key(),
            stats.tier.api_name(),
            stats.division.roman(),
            stats.lp,
            stats.wins,
            stats.losses,
            taken_at
        ],
    )?;
    Ok(())
}

/// Строка `rank_history` → точка. `Ok(None)` — незнакомые очередь, тир или дивизион.
fn point_from_row(row: &Row<'_>) -> AppResult<Option<RankPoint>> {
    let queue: String = row.get("queue")?;
    let tier: String = row.get("tier")?;
    let division: String = row.get("division")?;
    // Блок с `?` внутри замыкания-«try»: любой None → весь результат None.
    let parsed = (|| Some((Queue::from_key(&queue)?, Tier::from_api(&tier)?, Division::from_api(&division)?)))();
    let Some((queue, tier, division)) = parsed else {
        return Ok(None);
    };
    Ok(Some(RankPoint {
        queue,
        taken_at: row.get("taken_at")?,
        stats: RankedStats {
            tier,
            division,
            lp: row.get("lp")?,
            wins: row.get("wins")?,
            losses: row.get("losses")?,
        },
    }))
}

/// Riot ID сравниваем без учёта регистра (to_lowercase понимает и кириллицу).
fn same_riot_id(a: &RiotId, b: &RiotId) -> bool {
    a.game_name.to_lowercase() == b.game_name.to_lowercase()
        && a.tag_line.to_lowercase() == b.tag_line.to_lowercase()
}

/// Теги: обрезаем пробелы, выкидываем пустые и повторы (без учёта регистра), порядок сохраняем.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        let duplicate = out.iter().any(|t| t.to_lowercase() == tag.to_lowercase());
        if !tag.is_empty() && !duplicate {
            out.push(tag.to_string());
        }
    }
    out
}

/// Значение → JSON-строка для столбца. `T: Serialize` — любой тип, который serde умеет записать.
fn to_json<T: serde::Serialize>(value: &T) -> AppResult<String> {
    serde_json::to_string(value).map_err(|e| AppError::Database(e.to_string()))
}

fn tags_json(tags: &[String]) -> String {
    serde_json::to_string(&normalize_tags(tags)).expect("Vec<String> всегда сериализуется в JSON")
}

/// Текущее unix-время в секундах.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{Division, RankedStats, Tier};

    fn input(id: &str, region: Region) -> AccountInput {
        AccountInput {
            riot_id: RiotId::parse(id).unwrap(),
            region,
            note: "  заметка  ".into(),
            tags: vec!["main".into(), " Main ".into(), "".into(), "смурф".into()],
        }
    }

    fn snapshot(puuid: &str, riot_id: &str) -> AccountSnapshot {
        AccountSnapshot {
            source: DataSource::RiotApi,
            puuid: puuid.into(),
            riot_id: RiotId::parse(riot_id).unwrap(),
            region: Region::Euw,
            profile_icon_id: 588,
            summoner_level: 321,
            ranked: RankedQueues {
                solo: Some(RankedStats {
                    tier: Tier::Gold,
                    division: Division::Two,
                    lp: 45,
                    wins: 30,
                    losses: Some(25),
                }),
                flex: None,
            },
            matches: Some(vec![game("EUW1_2", true), game("EUW1_1", false)]),
            details: vec![],
        }
    }

    fn game(id: &str, win: bool) -> MatchSummary {
        // «EUW1_7» → сыграна в 1007: чем больше номер, тем новее.
        let number: i64 = id.rsplit('_').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        MatchSummary {
            id: id.into(),
            win,
            remake: false,
            queue_id: 420,
            played_at: 1000 + number,
            duration_secs: 1800,
            stats_version: crate::riot::models::MATCH_STATS_VERSION,
            me: crate::riot::PlayerStats {
                champion_id: 42,
                champion: "Корки".into(),
                kills: 1,
                deaths: 2,
                assists: 3,
                ..Default::default()
            },
        }
    }

    #[test]
    fn matches_are_cached_and_kept_when_not_loaded() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        db.save_snapshot(a.id, &snapshot("p", "Name#EUW"), 1).unwrap();
        let ids = |db: &Db| -> Vec<String> {
            let cache = db.get_account(a.id).unwrap().cache.unwrap();
            cache.matches.unwrap_or_default().into_iter().map(|m| m.id).collect()
        };
        assert_eq!(ids(&db), ["EUW1_2", "EUW1_1"]);

        // Игры не загрузились (None) — в кэше остаются прежние, ранги обновляются.
        let mut no_games = snapshot("p", "Name#EUW");
        no_games.matches = None;
        no_games.summoner_level = 400;
        db.save_snapshot(a.id, &no_games, 2).unwrap();
        assert_eq!(ids(&db), ["EUW1_2", "EUW1_1"]);
        assert_eq!(db.get_account(a.id).unwrap().cache.unwrap().summoner_level, 400);

        // Загрузились (пусть и пустой список) — заменяют прежние.
        no_games.matches = Some(vec![]);
        db.save_snapshot(a.id, &no_games, 3).unwrap();
        assert_eq!(db.get_account(a.id).unwrap().cache.unwrap().matches, Some(vec![]), "игр нет — но загружены");
    }

    #[test]
    fn rank_history_records_only_changes() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        let now = unix_now();
        db.save_snapshot(a.id, &snapshot("p", "Name#EUW"), now - 100).unwrap();
        db.save_snapshot(a.id, &snapshot("p", "Name#EUW"), now - 90).unwrap(); // ранг тот же

        let mut won = snapshot("p", "Name#EUW");
        if let Some(solo) = won.ranked.solo.as_mut() {
            solo.lp = 64;
            solo.wins = 31;
        }
        db.save_snapshot(a.id, &won, now - 50).unwrap();
        // Те же данные, но поражения скрыты (клиент League) — это не изменение.
        won.ranked.hide_losses();
        db.save_snapshot(a.id, &won, now - 40).unwrap();

        let points = |db: &Db| -> Vec<(i64, i32)> {
            let history = db.get_account(a.id).unwrap().history;
            assert!(history.iter().all(|p| p.queue == Queue::Solo), "Flex без ранга — точек нет");
            history.iter().map(|p| (p.taken_at, p.stats.lp)).collect()
        };
        assert_eq!(points(&db), [(now - 100, 45), (now - 50, 64)]);
        assert_eq!(db.list_accounts().unwrap()[0].history, db.get_account(a.id).unwrap().history);

        // Из давней истории загружается только последняя точка — начало периода.
        let old = RankedStats {
            tier: Tier::Silver,
            division: Division::One,
            lp: 10,
            wins: 1,
            losses: None,
        };
        insert_point(&db.conn, a.id, Queue::Solo, &old, now - 300 * DAY).unwrap();
        insert_point(&db.conn, a.id, Queue::Solo, &old, now - 200 * DAY).unwrap();
        assert_eq!(points(&db), [(now - 200 * DAY, 10), (now - 100, 45), (now - 50, 64)]);

        // Другой Riot ID — другой аккаунт: история стирается.
        db.update_account(a.id, &input("Other#EUW", Region::Euw)).unwrap();
        assert!(db.get_account(a.id).unwrap().history.is_empty());
    }

    /// Подробности игры для архива (одна команда, один игрок — для проверки хватит).
    fn details(id: &str) -> MatchDetails {
        MatchDetails {
            id: id.into(),
            queue_id: 420,
            played_at: 1000,
            duration_secs: 1800,
            remake: false,
            teams: vec![crate::riot::TeamDetails {
                win: true,
                towers: 9,
                dragons: 3,
                barons: 1,
                players: vec![crate::riot::PlayerLine {
                    riot_id: RiotId::parse("Name#EUW").ok(),
                    is_me: true,
                    stats: game(id, true).me,
                }],
            }],
        }
    }

    /// «Уже встречались»: игрок ищется по Riot ID в архивах всех аккаунтов, без учёта
    /// регистра; своя команда — первая; ремейки и сам владелец архива не считаются.
    #[test]
    fn meetings_are_found_in_every_archive() {
        let mut db = Db::open_in_memory().unwrap();
        let main = db.insert_account(&input("Main#EUW", Region::Euw)).unwrap();
        let alt = db.insert_account(&input("Alt#EUW", Region::Euw)).unwrap();
        let line = |name: &str, is_me: bool, champion: &str| crate::riot::PlayerLine {
            riot_id: RiotId::parse(name).ok(),
            is_me,
            stats: crate::riot::PlayerStats {
                champion: champion.into(),
                ..Default::default()
            },
        };
        let team = |win: bool, players| crate::riot::TeamDetails {
            win,
            towers: 0,
            dragons: 0,
            barons: 0,
            players,
        };
        let save = |db: &mut Db, account: AccountId, owner: &str, id: &str, remake: bool, ours_won: bool, pal: &str, foe: &str| {
            let mut snap = snapshot(&format!("p{account}"), owner);
            snap.matches = Some(vec![game(id, ours_won)]);
            db.save_snapshot(account, &snap, 1).unwrap();
            let details = MatchDetails {
                remake,
                teams: vec![
                    team(ours_won, vec![line(owner, true, "Корки"), line(pal, false, "Ари")]),
                    team(!ours_won, vec![line(foe, false, "Зед")]),
                ],
                ..details(id)
            };
            db.save_details(account, &details).unwrap();
        };
        save(&mut db, main.id, "Main#EUW", "EUW1_1", false, true, "Pal#EUW", "Foe#EUW");
        save(&mut db, main.id, "Main#EUW", "EUW1_2", true, true, "Pal#EUW", "Foe#EUW"); // ремейк
        save(&mut db, alt.id, "Alt#EUW", "EUW1_3", false, false, "Other#EUW", "pal#euw");

        let pal = RiotId::parse("PAL#euw").unwrap();
        let mut found = db.meetings(&[pal.clone(), RiotId::parse("Nobody#EUW").unwrap()]).unwrap();
        found.sort_by_key(|(_, m)| m.played_at);
        let seen: Vec<_> = found
            .iter()
            .map(|(who, m)| (who.clone(), m.match_id.as_str(), m.account.as_str(), m.same_team, m.won, m.champion.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                (pal.clone(), "EUW1_1", "Main#EUW", true, true, "Ари"),
                // На втором аккаунте он был против нас, и мы проиграли.
                (pal.clone(), "EUW1_3", "Alt#EUW", false, false, "Зед"),
            ]
        );
        // Владелец архива сам себе не «встреча».
        assert!(db.meetings(&[RiotId::parse("Main#EUW").unwrap()]).unwrap().is_empty());
        assert!(db.meetings(&[]).unwrap().is_empty());
    }

    /// Игроки игры: все, кроме самого аккаунта; своя команда — по отметке is_me (даже
    /// если она записана не первой); скрытые имена — `None`; игры других аккаунтов не попадают.
    #[test]
    fn archived_players_mark_allies() {
        let mut db = Db::open_in_memory().unwrap();
        let main = db.insert_account(&input("Main#EUW", Region::Euw)).unwrap();
        let alt = db.insert_account(&input("Alt#EUW", Region::Euw)).unwrap();
        let line = |name: Option<&str>, is_me: bool| crate::riot::PlayerLine {
            riot_id: name.and_then(|n| RiotId::parse(n).ok()),
            is_me,
            stats: crate::riot::PlayerStats {
                champion: format!("чемпион {}", name.unwrap_or("?")),
                ..Default::default()
            },
        };
        let team = |players| crate::riot::TeamDetails {
            win: true,
            towers: 0,
            dragons: 0,
            barons: 0,
            players,
        };
        let save = |db: &mut Db, account: AccountId, owner: &str, id: &str, teams| {
            let mut snap = snapshot(&format!("p{account}"), owner);
            snap.matches = Some(vec![game(id, true)]);
            db.save_snapshot(account, &snap, 1).unwrap();
            db.save_details(account, &MatchDetails { teams, ..details(id) }).unwrap();
        };
        save(
            &mut db,
            main.id,
            "Main#EUW",
            "EUW1_1",
            vec![
                team(vec![line(Some("Main#EUW"), true), line(Some("Pal#EUW"), false), line(None, false)]),
                team(vec![line(Some("Foe#EUW"), false)]),
            ],
        );
        // Своя команда записана второй.
        save(
            &mut db,
            main.id,
            "Main#EUW",
            "EUW1_2",
            vec![
                team(vec![line(Some("Foe#EUW"), false)]),
                team(vec![line(Some("Pal#EUW"), false), line(Some("Main#EUW"), true)]),
            ],
        );
        save(&mut db, alt.id, "Alt#EUW", "EUW1_3", vec![team(vec![line(Some("Alt#EUW"), true), line(Some("Other#EUW"), false)])]);

        let found = db.archived_players(main.id).unwrap();
        let player = |name: Option<&str>, ally: bool| ArchivedPlayer {
            riot_id: name.map(|n| RiotId::parse(n).unwrap()),
            champion: format!("чемпион {}", name.unwrap_or("?")),
            ally,
        };
        let sorted = |id: &str| {
            let mut list = found[id].clone();
            list.sort_by_key(|p| p.riot_id.as_ref().map(ToString::to_string));
            list
        };
        assert_eq!(found.len(), 2);
        assert_eq!(
            sorted("EUW1_1"),
            [player(None, true), player(Some("Foe#EUW"), false), player(Some("Pal#EUW"), true)]
        );
        assert_eq!(sorted("EUW1_2"), [player(Some("Foe#EUW"), false), player(Some("Pal#EUW"), true)]);
        assert!(db.archived_players(12345).unwrap().is_empty());
    }

    #[test]
    fn settings_by_prefix_and_inventory_cleanup() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        db.set_setting(&format!("inventory_{}", a.id), "{}").unwrap();
        db.set_setting("inventoryXlike", "не подходит: после inventory нет «_»").unwrap();
        db.set_setting("sort", "rank").unwrap();
        let keys: Vec<String> = db.settings_with_prefix("inventory_").unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [format!("inventory_{}", a.id)]);
        db.delete_account(a.id).unwrap();
        assert!(db.settings_with_prefix("inventory_").unwrap().is_empty(), "инвентарь удаляется вместе с аккаунтом");
    }

    /// Позиция, которую узнали во время игры, не стирается, когда клиент потом
    /// присылает ту же игру без позиции.
    #[test]
    fn archive_keeps_a_known_position() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        let mut with_position = game("EUW1_1", true);
        with_position.me.position = "middle".into();
        let mut snap = snapshot("p", "Name#EUW");
        snap.matches = Some(vec![with_position]);
        db.save_snapshot(a.id, &snap, 1).unwrap();
        snap.matches = Some(vec![game("EUW1_1", false)]);
        db.save_snapshot(a.id, &snap, 2).unwrap();
        let saved = &db.archived_matches(a.id, None).unwrap()[0];
        assert_eq!(saved.me.position, "middle");
        assert!(!saved.win, "остальное обновилось");
    }

    /// Время ранговых игр (для оценки сгорания ранга): только 420 и 440, без ремейков
    /// и без давних игр, от старых к новым.
    #[test]
    fn ranked_games_come_from_the_archive() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        let now = unix_now();
        let played = |id: &str, queue_id: i32, days_ago: i64, remake: bool| MatchSummary {
            queue_id,
            remake,
            played_at: now - days_ago * DAY,
            ..game(id, true)
        };
        let mut snap = snapshot("p", "Name#EUW");
        snap.matches = Some(vec![
            played("EUW1_1", 420, 1, false),
            played("EUW1_2", 450, 2, false), // ARAM
            played("EUW1_3", 440, 3, false),
            played("EUW1_4", 420, 4, true), // ремейк
            played("EUW1_5", 420, 5, false),
            played("EUW1_6", 420, RANKED_GAMES_DAYS + 5, false), // слишком давно
        ]);
        db.save_snapshot(a.id, &snap, now).unwrap();
        let expected = vec![(Queue::Solo, now - 5 * DAY), (Queue::Flex, now - 3 * DAY), (Queue::Solo, now - DAY)];
        assert_eq!(db.get_account(a.id).unwrap().ranked_games, expected);
        assert_eq!(db.list_accounts().unwrap()[0].ranked_games, expected);
    }

    #[test]
    fn archive_keeps_every_game() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        // Первое обновление: игры 1–7 (больше, чем показывается на строке).
        let mut first = snapshot("p", "Name#EUW");
        first.matches = Some((1..=7).rev().map(|n| game(&format!("EUW1_{n}"), true)).collect());
        first.details = vec![details("EUW1_7")];
        db.save_snapshot(a.id, &first, 1).unwrap();
        // Второе: сервер отдаёт только последние — 6–9. Старые 1–5 из архива не пропадают.
        let mut second = snapshot("p", "Name#EUW");
        second.matches = Some((6..=9).rev().map(|n| game(&format!("EUW1_{n}"), false)).collect());
        db.save_snapshot(a.id, &second, 2).unwrap();

        let all = db.archived_matches(a.id, None).unwrap();
        let ids: Vec<&str> = all.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["EUW1_9", "EUW1_8", "EUW1_7", "EUW1_6", "EUW1_5", "EUW1_4", "EUW1_3", "EUW1_2", "EUW1_1"]);
        assert!(!all[2].win, "повторно пришедшая игра обновилась, а не задвоилась");
        assert_eq!(db.get_account(a.id).unwrap().archived_games, 9);
        assert_eq!(db.archived_matches(a.id, Some(2)).unwrap().len(), 2);

        // Подробности: пришли с первым обновлением только у 7-й, остальные — догружаются.
        assert_eq!(db.archived_details(a.id, "EUW1_7").unwrap(), Some(details("EUW1_7")));
        assert_eq!(db.archived_details(a.id, "EUW1_9").unwrap(), None);
        let missing = db.matches_without_details(a.id, 3).unwrap();
        assert_eq!(missing, ["EUW1_9", "EUW1_8", "EUW1_6"]);
        db.save_details(a.id, &details("EUW1_9")).unwrap();
        assert!(db.archived_details(a.id, "EUW1_9").unwrap().is_some());

        // Другой Riot ID — другой аккаунт: архив стирается.
        db.update_account(a.id, &input("Other#EUW", Region::Euw)).unwrap();
        assert!(db.archived_matches(a.id, None).unwrap().is_empty());
    }

    #[test]
    fn games_from_old_cache_move_to_archive() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        db.save_snapshot(a.id, &snapshot("p", "Name#EUW"), 1).unwrap();
        // Как будто база старая: архив пуст, игры лежат только в кэше.
        db.conn.execute("DELETE FROM matches", []).unwrap();
        db.archive_cached_matches().unwrap();
        let ids: Vec<String> = db.archived_matches(a.id, None).unwrap().into_iter().map(|g| g.id).collect();
        assert_eq!(ids, ["EUW1_2", "EUW1_1"]);
    }

    #[test]
    fn migrations_set_schema_version() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn insert_and_list() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Ёжик#RU1", Region::Ru)).unwrap();
        assert_eq!(a.riot_id.to_string(), "Ёжик#RU1");
        assert_eq!(a.note, "заметка");
        assert_eq!(a.tags, vec!["main", "смурф"]);
        assert!(!a.has_login && !a.has_password);
        assert_eq!(a.puuid, None);
        assert_eq!(a.cache, None);
        assert_eq!(a.secret_ref.len(), 36, "UUID");

        let all = db.list_accounts().unwrap();
        assert_eq!(all, vec![a]);
    }

    #[test]
    fn duplicates_are_rejected_case_insensitively() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_account(&input("Ёжик#RU1", Region::Ru)).unwrap();
        let err = db.insert_account(&input("ЁЖИК#ru1", Region::Ru)).unwrap_err();
        assert_eq!(err, AppError::DuplicateAccount);
        // Тот же Riot ID в другом регионе — можно.
        db.insert_account(&input("Ёжик#RU1", Region::Euw)).unwrap();
    }

    #[test]
    fn snapshot_is_cached_and_renames_account() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Old#EUW", Region::Euw)).unwrap();
        db.save_snapshot(a.id, &snapshot("puuid-1", "New Name#EUW"), 1000).unwrap();

        let a = db.get_account(a.id).unwrap();
        assert_eq!(a.puuid.as_deref(), Some("puuid-1"));
        assert_eq!(a.riot_id.to_string(), "New Name#EUW");
        let cache = a.cache.expect("кэш");
        assert_eq!(cache.summoner_level, 321);
        assert_eq!(cache.ranked.solo.unwrap().rank_label(), "Золото II");
        assert_eq!(cache.fetched_at, 1000);

        // Повторное сохранение обновляет кэш, а не добавляет строку.
        db.save_snapshot(a.id, &snapshot("puuid-1", "New Name#EUW"), 2000).unwrap();
        assert_eq!(db.get_account(a.id).unwrap().cache.unwrap().fetched_at, 2000);
    }

    #[test]
    fn same_puuid_on_two_accounts_is_a_duplicate() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("One#EUW", Region::Euw)).unwrap();
        let b = db.insert_account(&input("Two#EUW", Region::Euw)).unwrap();
        db.save_snapshot(a.id, &snapshot("same", "One#EUW"), 1).unwrap();
        let err = db.save_snapshot(b.id, &snapshot("same", "One#EUW"), 1).unwrap_err();
        assert_eq!(err, AppError::DuplicateAccount);
    }

    #[test]
    fn lcu_and_api_puuids_are_stored_separately() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#RU1", Region::Ru)).unwrap();
        db.save_snapshot(a.id, &snapshot("api-puuid", "Name#RU1"), 1).unwrap();
        let lcu = AccountSnapshot {
            source: DataSource::Lcu,
            ..snapshot("lcu-puuid", "Name#RU1")
        };
        db.save_snapshot(a.id, &lcu, 2).unwrap();

        let a = db.get_account(a.id).unwrap();
        assert_eq!(a.puuid.as_deref(), Some("api-puuid"), "PUUID API не затёрт");
        assert_eq!(a.lcu_puuid.as_deref(), Some("lcu-puuid"));
        assert_eq!(a.cache.unwrap().fetched_at, 2);

        // Тот же LCU PUUID у другого аккаунта — дубликат.
        let b = db.insert_account(&input("Other#RU1", Region::Ru)).unwrap();
        assert_eq!(db.save_snapshot(b.id, &lcu, 3).unwrap_err(), AppError::DuplicateAccount);

        // Смена региона сбрасывает оба PUUID.
        let a = db.update_account(a.id, &input("Name#RU1", Region::Euw)).unwrap();
        assert!(a.puuid.is_none() && a.lcu_puuid.is_none());
    }

    #[test]
    fn changing_riot_id_drops_puuid_and_cache() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        db.save_snapshot(a.id, &snapshot("p", "Name#EUW"), 1).unwrap();

        // Только регистр — тот же аккаунт, кэш остаётся.
        let a = db.update_account(a.id, &input("NAME#euw", Region::Euw)).unwrap();
        assert!(a.cache.is_some() && a.puuid.is_some());

        // Другой регион — кэш и PUUID сбрасываются.
        let a = db.update_account(a.id, &input("NAME#euw", Region::Eune)).unwrap();
        assert!(a.cache.is_none() && a.puuid.is_none());
    }

    #[test]
    fn delete_cascades_to_cache() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_account(&input("Name#EUW", Region::Euw)).unwrap();
        db.save_snapshot(a.id, &snapshot("p", "Name#EUW"), 1).unwrap();
        db.delete_account(a.id).unwrap();
        assert_eq!(db.get_account(a.id).unwrap_err(), AppError::AccountMissing);
        let left: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM account_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert_eq!(db.delete_account(a.id).unwrap_err(), AppError::AccountMissing);
    }

    #[test]
    fn settings_roundtrip() {
        let mut db = Db::open_in_memory().unwrap();
        assert_eq!(db.setting("ddragon_version").unwrap(), None);
        db.set_setting("ddragon_version", "16.21.1").unwrap();
        db.set_setting("ddragon_version", "16.22.1").unwrap();
        assert_eq!(db.setting("ddragon_version").unwrap().as_deref(), Some("16.22.1"));
        db.delete_setting("ddragon_version").unwrap();
        assert_eq!(db.setting("ddragon_version").unwrap(), None);
    }
}
