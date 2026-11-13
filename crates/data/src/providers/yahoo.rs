use crate::db::{DailyBar, LatestQuoteRecord};
use crate::error::DataError;
use crate::providers::PriceProvider;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use chrono_tz::Europe::Rome;
use pac_core::types::Currency;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;

/// Provider isolato per Yahoo Finance (endpoint v8 chart non ufficiale),
/// dotato di timeout, retry con backoff esponenziale e User-Agent configurabile.
pub struct YahooFinanceProvider {
    http: reqwest::Client,
    base_url: String,
    max_retries: u32,
}

impl YahooFinanceProvider {
    pub fn new(user_agent: &str, timeout_secs: u64, max_retries: u32) -> Result<Self, DataError> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(timeout_secs))
            .build()?;
        Ok(Self {
            http,
            base_url: "https://query1.finance.yahoo.com".to_string(),
            max_retries,
        })
    }

    pub fn with_base_url(
        user_agent: &str,
        base_url: String,
        timeout_secs: u64,
        max_retries: u32,
    ) -> Result<Self, DataError> {
        let mut p = Self::new(user_agent, timeout_secs, max_retries)?;
        p.base_url = base_url;
        Ok(p)
    }

    async fn fetch_chart_json(&self, symbol: &str, range: &str) -> Result<YahooChartResponse, DataError> {
        let url = format!(
            "{}/v8/finance/chart/{}?interval=1d&range={}",
            self.base_url.trim_end_matches('/'),
            symbol,
            range
        );

        let mut attempt = 0_u32;
        loop {
            let resp = self.http.get(&url).send().await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    let parsed: YahooChartResponse = r.json().await?;
                    return Ok(parsed);
                }
                Ok(r) => {
                    let status = r.status();
                    if attempt >= self.max_retries {
                        return Err(DataError::Parse {
                            provider: "YahooFinance".to_string(),
                            details: format!("HTTP {status} per {symbol}"),
                        });
                    }
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(DataError::Http(e));
                    }
                }
            }
            attempt += 1;
            let backoff_ms = 250_u64 * (1_u64 << attempt.min(5));
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    }
}

#[async_trait]
impl PriceProvider for YahooFinanceProvider {
    fn name(&self) -> &'static str {
        "YahooFinance"
    }

    async fn fetch_latest_quote(
        &self,
        symbol: &str,
        expected_currency: Currency,
    ) -> Result<LatestQuoteRecord, DataError> {
        let chart = self.fetch_chart_json(symbol, "5d").await?;
        let result = chart
            .chart
            .result
            .and_then(|mut v| v.pop())
            .ok_or_else(|| DataError::NotFound {
                symbol: symbol.to_string(),
            })?;

        let raw_price = result
            .meta
            .regular_market_price
            .ok_or_else(|| DataError::NotFound {
                symbol: symbol.to_string(),
            })?;

        let price_native = Decimal::from_str(&raw_price.to_string()).map_err(|e| DataError::Parse {
            provider: "YahooFinance".to_string(),
            details: e.to_string(),
        })?;

        let ts_rome = result
            .meta
            .regular_market_time
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .unwrap_or_else(Utc::now)
            .with_timezone(&Rome);

        Ok(LatestQuoteRecord {
            symbol: symbol.to_string(),
            price_native,
            currency: expected_currency,
            source: "Yahoo Finance".to_string(),
            timestamp_rome: ts_rome,
        })
    }

    async fn fetch_daily_history(
        &self,
        symbol: &str,
        expected_currency: Currency,
        days: u32,
    ) -> Result<Vec<DailyBar>, DataError> {
        let range = if days > 365 { "2y" } else { "1y" };
        let chart = self.fetch_chart_json(symbol, range).await?;
        let result = chart
            .chart
            .result
            .and_then(|mut v| v.pop())
            .ok_or_else(|| DataError::NotFound {
                symbol: symbol.to_string(),
            })?;

        let timestamps = result.timestamp.unwrap_or_default();
        let closes = result
            .indicators
            .quote
            .into_iter()
            .next()
            .and_then(|q| q.close)
            .unwrap_or_default();

        let mut bars = Vec::new();
        for (ts, close_opt) in timestamps.into_iter().zip(closes) {
            let Some(val) = close_opt else { continue };
            let Some(dt) = DateTime::<Utc>::from_timestamp(ts, 0) else {
                continue;
            };
            let Ok(dec_val) = Decimal::from_str(&val.to_string()) else {
                continue;
            };
            bars.push(DailyBar {
                date: dt.with_timezone(&Rome).date_naive(),
                close_native: dec_val,
                currency: expected_currency,
                source: "Yahoo Finance".to_string(),
            });
        }
        Ok(bars)
    }
}

#[derive(Debug, Deserialize)]
struct YahooChartResponse {
    chart: YahooChartBody,
}

#[derive(Debug, Deserialize)]
struct YahooChartBody {
    result: Option<Vec<YahooChartResult>>,
}

#[derive(Debug, Deserialize)]
struct YahooChartResult {
    meta: YahooMeta,
    timestamp: Option<Vec<i64>>,
    indicators: YahooIndicators,
}

#[derive(Debug, Deserialize)]
struct YahooMeta {
    #[serde(rename = "regularMarketPrice")]
    regular_market_price: Option<serde_json::Number>,
    #[serde(rename = "regularMarketTime")]
    regular_market_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct YahooIndicators {
    quote: Vec<YahooQuoteSeries>,
}

#[derive(Debug, Deserialize)]
struct YahooQuoteSeries {
    close: Option<Vec<Option<serde_json::Number>>>,
}