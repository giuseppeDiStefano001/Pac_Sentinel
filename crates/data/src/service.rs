use crate::db::{LatestQuoteRecord, PortfolioRepository};
use crate::error::DataError;
use crate::market_hours::now_rome;
use crate::providers::bce::BceFxClient;
use crate::providers::FallbackPriceChain;
use pac_core::config::PortfolioConfig;
use pac_core::types::{AssetId, Currency, MarketSnapshot};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;
use tracing::{info, warn};

/// Servizio applicativo che coordina sincronizzazione BCE, storico a 400 giorni e fallback provider.
pub struct MarketDataService<'a> {
    repo: &'a PortfolioRepository,
    chain: &'a FallbackPriceChain,
    bce: &'a BceFxClient,
}

impl<'a> MarketDataService<'a> {
    pub fn new(
        repo: &'a PortfolioRepository,
        chain: &'a FallbackPriceChain,
        bce: &'a BceFxClient,
    ) -> Self {
        Self { repo, chain, bce }
    }

    /// Aggiorna il cambio EUR/USD ufficiale BCE e restituisce il tasso attivo (con fallback su ultimo DB).
    pub async fn sync_eur_usd_rate(&self) -> Result<Decimal, DataError> {
        match self.bce.fetch_eur_usd().await {
            Ok(fx) => {
                self.repo.upsert_fx_rate(fx.date, fx.eur_usd, "BCE eurofxref")?;
                info!(date = %fx.date, rate = %fx.eur_usd, "Cambio EUR/USD BCE aggiornato");
                Ok(fx.eur_usd)
            }
            Err(err) => {
                warn!(error = %err, "Feed BCE non raggiungibile, uso ultimo cambio salvato nel DB");
                if let Some((_, rate, _)) = self.repo.latest_eur_usd()? {
                    Ok(rate)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Garantisce che nel DB siano presenti almeno 400 giorni di storico giornaliero per ogni strumento
    /// e aggiorna le quotazioni correnti tramite la catena di fallback.
    pub async fn refresh_all_market_data(
        &self,
        config: &PortfolioConfig,
    ) -> Result<BTreeMap<AssetId, MarketSnapshot>, DataError> {
        let now = now_rome();
        let eur_usd = self.sync_eur_usd_rate().await?;

        for asset in AssetId::ALL {
            let (symbol, currency) = match asset {
                AssetId::EtfSp500 => (config.market.etf_symbol.as_str(), Currency::Eur),
                AssetId::Nvda => ("NVDA", Currency::Usd),
                AssetId::Mp => ("MP", Currency::Usd),
            };

            // Bootstrap storico se abbiamo meno di 250 sedute salvate
            let existing_bars = self.repo.history_count(symbol)?;
            if existing_bars < 250 {
                if let Ok(bars) = self
                    .chain
                    .fetch_history_with_fallback(
                        symbol,
                        currency,
                        config.market.history_days_initial_fetch,
                    )
                    .await
                {
                    self.repo.upsert_daily_bars(symbol, &bars, now)?;
                }
            }

            // Aggiorna quotazione spot tramite catena di fallback
            if let Ok(quote) = self.chain.fetch_latest_with_fallback(symbol, currency).await {
                self.repo.upsert_latest_quote(&quote)?;
            }
        }

        self.repo.build_all_snapshots(
            &config.market.etf_symbol,
            eur_usd,
            now,
            config.market.stale_after_hours,
        )
    }

    /// Quarto livello di fallback: inserimento manuale di un prezzo da parte dell'utente.
    pub fn record_manual_price(
        &self,
        symbol: &str,
        price_native: Decimal,
        currency: Currency,
    ) -> Result<(), DataError> {
        if price_native <= dec!(0) {
            return Err(DataError::Parse {
                provider: "ManualInput".to_string(),
                details: "Il prezzo manuale deve essere positivo".to_string(),
            });
        }
        self.repo.upsert_latest_quote(&LatestQuoteRecord {
            symbol: symbol.to_string(),
            price_native,
            currency,
            source: "Inserimento manuale".to_string(),
            timestamp_rome: now_rome(),
        })
    }
}