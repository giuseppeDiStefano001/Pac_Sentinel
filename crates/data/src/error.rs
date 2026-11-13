use pac_core::error::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("Errore dominio: {0}")]
    Core(#[from] CoreError),

    #[error("Errore database SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Errore rete HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Errore parsing dati ({provider}): {details}")]
    Parse { provider: String, details: String },

    #[error("Tutti i provider hanno fallito per lo strumento {symbol}: {details}")]
    AllProvidersFailed { symbol: String, details: String },

    #[error("Dato non disponibile per {symbol}")]
    NotFound { symbol: String },
}