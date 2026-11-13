pub mod api_key;
pub mod bce;
pub mod stooq;
pub mod yahoo;

use crate::db::{DailyBar, LatestQuoteRecord};
use crate::error::DataError;
use async_trait::async_trait;
use pac_core::types::Currency;
use tracing::{info, warn};

/// Interfaccia astratta per i provider di quotazioni e storico.
#[async_trait]
pub trait PriceProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Recupera l'ultima quotazione disponibile per il simbolo.
    async fn fetch_latest_quote(
        &self,
        symbol: &str,
        expected_currency: Currency,
    ) -> Result<LatestQuoteRecord, DataError>;

    /// Recupera lo storico giornaliero di chiusura (almeno `days` giorni solari).
    async fn fetch_daily_history(
        &self,
        symbol: &str,
        expected_currency: Currency,
        days: u32,
    ) -> Result<Vec<DailyBar>, DataError>;
}

/// Esegue la catena di fallback ordinata su più `PriceProvider`.
pub struct FallbackPriceChain {
    providers: Vec<Box<dyn PriceProvider>>,
}

impl FallbackPriceChain {
    pub fn new(providers: Vec<Box<dyn PriceProvider>>) -> Self {
        Self { providers }
    }

    pub async fn fetch_latest_with_fallback(
        &self,
        symbol: &str,
        expected_currency: Currency,
    ) -> Result<LatestQuoteRecord, DataError> {
        let mut errors = Vec::new();
        for provider in &self.providers {
            match provider.fetch_latest_quote(symbol, expected_currency).await {
                Ok(quote) => {
                    info!(provider = provider.name(), symbol = symbol, "Quotazione recuperata");
                    return Ok(quote);
                }
                Err(err) => {
                    warn!(
                        provider = provider.name(),
                        symbol = symbol,
                        error = %err,
                        "Fallback al provider successivo"
                    );
                    errors.push(format!("{}: {err}", provider.name()));
                }
            }
        }
        Err(DataError::AllProvidersFailed {
            symbol: symbol.to_string(),
            details: errors.join(" | "),
        })
    }

    pub async fn fetch_history_with_fallback(
        &self,
        symbol: &str,
        expected_currency: Currency,
        days: u32,
    ) -> Result<Vec<DailyBar>, DataError> {
        let mut errors = Vec::new();
        for provider in &self.providers {
            match provider
                .fetch_daily_history(symbol, expected_currency, days)
                .await
            {
                Ok(bars) if !bars.is_empty() => {
                    info!(
                        provider = provider.name(),
                        symbol = symbol,
                        bars = bars.len(),
                        "Storico giornaliero recuperato"
                    );
                    return Ok(bars);
                }
                Ok(_) => {
                    errors.push(format!("{}: storico vuoto", provider.name()));
                }
                Err(err) => {
                    warn!(
                        provider = provider.name(),
                        symbol = symbol,
                        error = %err,
                        "Fallback storico al provider successivo"
                    );
                    errors.push(format!("{}: {err}", provider.name()));
                }
            }
        }
        Err(DataError::AllProvidersFailed {
            symbol: symbol.to_string(),
            details: errors.join(" | "),
        })
    }
}