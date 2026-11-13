//! Client Telegram Bot API (HTML), formattazione messaggi di segnale,
//! motore anti-spam e gestore comandi di sola lettura.

pub mod client;
pub mod commands;
pub mod error;
pub mod formatter;
pub mod policy;

pub use error::NotifyError;