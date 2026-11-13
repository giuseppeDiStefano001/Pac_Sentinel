use crate::db::{DailyBar, LatestQuoteRecord};
use crate::error::DataError;
use crate::market_hours::now_rome;
use crate::providers::PriceProvider;
use async_trait::async_trait;
use chrono::NaiveDate;
use pac_core::types::Currency;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiProviderKind {
    TwelveData,
    Finnhub,
}

/// Terzo provider nella catena di fallback: usa una chiave API recuperata dal keyring OS.
pub struct ApiKeyMarketProvider {
    http: reqwest::Client,
    kind: ApiProviderKind,
    api_key: String,
    base_url: String,
}

impl ApiKeyMarketProvider {
    pub fn new(http: reqwest::Client, kind: ApiProviderKind, api_key: String) -> Self {
        let base_url = match kind {
            ApiProviderKind::TwelveData => "https://api.twelvedata.com".to_string(),
            ApiProviderKind::Finnhub => "https://finnhub.io/api/v1".to_string(),
        };
        Self {
            http,
            kind,
            api_key,
            base_url,
        }
    }

    pub fn with_base_url(
        http: reqwest::Client,
        kind: ApiProviderKind,
        api_key: String,
        base_url: String,
    ) -> Self {
        Self {
            http,
            kind,
            api_key,
            base_url,
        }
    }
}

#[async_trait]
impl PriceProvider for ApiKeyMarketProvider {
    fn name(&self) -> &'static str {
        match self.kind {
            ApiProviderKind::TwelveData => "TwelveData",
            ApiProviderKind::Finnhub => "Finnhub",
        }
    }

    async fn fetch_latest_quote(
        &self,
        symbol: &str,
        expected_currency: Currency,
    ) -> Result<LatestQuoteRecord, DataError> {
        if self.api_key.trim().is_empty() {
            return Err(DataError::Parse {
                provider: self.name().to_string(),
                details: "Chiave API non configurata nel keyring".to_string(),
            });
        }

        match self.kind {
            ApiProviderKind::TwelveData => {
                let url = format!(
                    "{}/price?symbol={}&apikey={}",
                    self.base_url.trim_end_matches('/'),
                    symbol,
                    self.api_key
                );
                let resp: TwelveDataPriceResp =
                    self.http.get(&url).send().await?.error_for_status()?.json().await?;
                let price = Decimal::from_str(&resp.price).map_err(|e| DataError::Parse {
                    provider: "TwelveData".to_string(),
                    details: e.to_string(),
                })?;
                Ok(LatestQuoteRecord {
                    symbol: symbol.to_string(),
                    price_native: price,
                    currency: expected_currency,
                    source: "Twelve Data API".to_string(),
                    timestamp_rome: now_rome(),
                })
            }
            ApiProviderKind::Finnhub => {
                let url = format!(
                    "{}/quote?symbol={}&token={}",
                    self.base_url.trim_end_matches('/'),
                    symbol,
                    self.api_key
                );
                let resp: FinnhubQuoteResp =
                    self.http.get(&url).send().await?.error_for_status()?.json().await?;
                let price = Decimal::from_str(&resp.c.to_string()).map_err(|e| DataError::Parse {
                    provider: "Finnhub".to_string(),
                    details: e.to_string(),
                })?;
                Ok(LatestQuoteRecord {
                    symbol: symbol.to_string(),
                    price_native: price,
                    currency: expected_currency,
                    source: "Finnhub API".to_string(),
                    timestamp_rome: now_rome(),
                })
            }
        }
    }

    async fn fetch_daily_history(
        &self,
        symbol: &str,
        expected_currency: Currency,
        days: u32,
    ) -> Result<Vec<DailyBar>, DataError> {
        if self.api_key.trim().is_empty() {
            return Err(DataError::Parse {
                provider: self.name().to_string(),
                details: "Chiave API non configurata nel keyring".to_string(),
            });
        }

        if self.kind == ApiProviderKind::TwelveData {
            let url = format!(
                "{}/time_series?symbol={}&interval=1day&outputsize={}&apikey={}",
                self.base_url.trim_end_matches('/'),
                symbol,
                days,
                self.api_key
            );
            let resp: TwelveDataSeriesResp =
                self.http.get(&url).send().await?.error_for_status()?.json().await?;
            let mut bars = Vec::new();
            for item in resp.values.unwrap_or_default() {
                if let (Ok(date), Ok(close)) = (
                    NaiveDate::parse_from_str(&item.datetime, "%Y-%m-%d"),
                    Decimal::from_str(&item.close),
                ) {
                    bars.push(DailyBar {
                        date,
                        close_native: close,
                        currency: expected_currency,
                        source: "Twelve Data API".to_string(),
                    });
                }
            }
            bars.sort_by_key(|b| b.date);
            return Ok(bars);
        }

        Err(DataError::Parse {
            provider: "Finnhub".to_string(),
            details: "Storico candle non abilitato su piano free Finnhub".to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct TwelveDataPriceResp {
    price: String,
}

#[derive(Debug, Deserialize)]
struct TwelveDataSeriesResp {
    values: Option<Vec<TwelveDataBar>>,
}

#[derive(Debug, Deserialize)]
struct TwelveDataBar {
    datetime: String,
    close: String,
}

#[derive(Debug, Deserialize)]
struct FinnhubQuoteResp {
    c: serde_json::Number,
}