//! Работа с Riot API: маршрутизация по регионам, модели ответов, HTTP-клиент.

pub mod client;
pub mod models;
pub mod routing;

// Реэкспорт: снаружи можно писать `riot::RiotClient` вместо `riot::client::RiotClient`.
pub use client::{ApiKey, RiotClient};
pub use models::{
    AccountSnapshot, DataSource, Division, MatchDetails, MatchSummary, PlayerLine, PlayerStats, RankedQueues,
    RankedStats, TeamDetails, Tier,
};
pub use routing::{Region, RegionalRoute, RiotId};
