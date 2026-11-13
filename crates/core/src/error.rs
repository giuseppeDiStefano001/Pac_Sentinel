use rust_decimal::Decimal;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("Configurazione non valida: {0}")]
    InvalidConfig(String),

    #[error("Somma dei pesi target non valida: atteso 100%, trovato {0}%")]
    InvalidTargetWeights(Decimal),

    #[error("Giorno del PAC non valido ({0}): deve essere compreso tra 1 e 28")]
    InvalidPacDay(u8),

    #[error("Numero mese non valido: {0} (deve essere >= 1)")]
    InvalidMonthNumber(u32),

    #[error("Valore negativo non ammesso per {field}: {value}")]
    NegativeValue { field: &'static str, value: Decimal },

    #[error("Quantità da vendere ({requested}) superiore alla quantità posseduta ({available})")]
    InsufficientQuantity {
        requested: Decimal,
        available: Decimal,
    },

    #[error("Tasso di cambio EUR/USD non valido: {0}")]
    InvalidExchangeRate(Decimal),

    #[error("Errore parsing configurazione TOML: {0}")]
    TomlParse(String),
}