//! Livello di persistenza SQLite e acquisizione dati di mercato con fallback.

pub mod db;
pub mod error;
pub mod market_hours;
pub mod providers;
pub mod service;

pub use error::DataError;