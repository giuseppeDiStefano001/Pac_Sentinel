use crate::db::{DailyBar, LatestQuoteRecord};
use crate::error::DataError;
use crate::providers::PriceProvider;
use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Europe::Rome;
use pac_core::types::Currency;
use rust_decimal::Decimal;
use std::str::FromStr;

/// Provider Stooq basato su download CSV pubblico (secondo anello della catena di fallback).
pub struct StooqProvider {
    http: reqwest::Client,
    base_url: String,
}

impl StooqProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            base_url: "https://stooq.com".to_string(),
        }
    }

    pub fn with_base_url(http: reqwest::Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    fn map_stooq_symbol(symbol: &str) -> String {
        let lower = symbol.to_ascii_lowercase();
        if lower.contains('.') {
            lower
        } else {
            format!("{lower}.us")
        }
    }
}

#[async_trait]
impl PriceProvider for StooqProvider {
    fn name(&self) -> &'static str {
        "StooqCSV"
    }

    async fn fetch_latest_quote(
        &self,
        symbol: &str,
        expected_currency: Currency,
    ) -> Result<LatestQuoteRecord, DataError> {
        let bars = self.fetch_daily_history(symbol, expected_currency, 10).await?;
        let last = bars.last().ok_or_else(|| DataError::NotFound {
            symbol: symbol.to_string(),
        })?;
        let naive_dt = last
            .date
            .and_time(NaiveTime::from_hms_opt(17, 0, 0).unwrap_or(NaiveTime::MIN));
        let ts_rome = Rome
            .from_local_datetime(&naive_dt)
            .single()
            .unwrap_or_else(crate::market_hours::now_rome);

        Ok(LatestQuoteRecord {
            symbol: symbol.to_string(),
            price_native: last.close_native,
            currency: expected_currency,
            source: "Stooq CSV".to_string(),
            timestamp_rome: ts_rome,
        })
    }

    async fn fetch_daily_history(
        &self,
        symbol: &str,
        expected_currency: Currency,
        days: u32,
    ) -> Result<Vec<DailyBar>, DataError> {
        let stooq_sym = Self::map_stooq_symbol(symbol);
        let url = format!(
            "{}/q/d/l/?s={}&i=d",
            self.base_url.trim_end_matches('/'),
            stooq_sym
        );

        let csv_body = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(csv_body.as_bytes());

        let mut bars = Vec::new();
        for record in reader.records() {
            let rec = record.map_err(|e| DataError::Parse {
                provider: "StooqCSV".to_string(),
                details: e.to_string(),
            })?;
            // Colonne standard Stooq: Date,Open,High,Low,Close,Volume
            let Some(date_str) = rec.get(0) else { continue };
            let Some(close_str) = rec.get(4) else { continue };
            let Ok(date) = NaiveDate::parse_from_str(date_str.trim(), "%Y-%m-%d") else {
                continue;
            };
            let Ok(close) = Decimal::from_str(close_str.trim()) else {
                continue;
            };
            bars.push(DailyBar {
                date,
                close_native: close,
                currency: expected_currency,
                source: "Stooq CSV".to_string(),
            });
        }

        bars.sort_by_key(|b| b.date);
        let max_keep = usize::try_from(days).unwrap_or(400);
        if bars.len() > max_keep {
            bars = bars.split_off(bars.len() - max_keep);
        }
        Ok(bars)
    }
}