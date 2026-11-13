use pac_core::error::CoreError;
use pac_data::DataError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NotifyError {
    #[error("Errore dominio: {0}")]
    Core(#[from] CoreError),

    #[error("Errore persistenza/dati: {0}")]
    Data(#[from] DataError),

    #[error("Errore trasporto HTTP Telegram: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Risposta API Telegram non valida (status {status}): {description}")]
    TelegramApi { status: u16, description: String },

    #[error("Token Telegram non configurato nel keyring")]
    MissingBotToken,

    #[error("Chat ID autorizzato non configurato")]
    MissingAuthorizedChatId,
}