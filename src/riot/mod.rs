//! Работа с Riot API: маршрутизация по регионам, модели ответов, HTTP-клиент.

pub mod client;
pub mod models;
pub mod routing;

// Реэкспорт: снаружи можно писать `riot::RiotClient` вместо `riot::client::RiotClient`.
pub use client::{ApiKey, RiotClient};
pub use models::{
    AccountSnapshot, DataSource, Division, Inventory, MatchDetails, MatchSummary, OwnedChampion, OwnedSkin, PlayerLine,
    PlayerStats, RankedQueues, RankedStats, Restriction, RestrictionKind, TeamDetails, Tier,
};
pub use routing::{Region, RegionalRoute, RiotId};
