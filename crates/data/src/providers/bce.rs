use crate::error::DataError;
use chrono::NaiveDate;
use quick_xml::events::Event;
use quick_xml::Reader;
use rust_decimal::Decimal;
use std::str::FromStr;

pub const BCE_DAILY_XML_URL: &str =
    "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BceFxQuote {
    pub date: NaiveDate,
    pub eur_usd: Decimal,
}

/// Client per il feed ufficiale giornaliero della Banca Centrale Europea (eurofxref).
pub struct BceFxClient {
    http: reqwest::Client,
    url: String,
}

impl BceFxClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            url: BCE_DAILY_XML_URL.to_string(),
        }
    }

    pub fn with_custom_url(http: reqwest::Client, url: String) -> Self {
        Self { http, url }
    }

    pub async fn fetch_eur_usd(&self) -> Result<BceFxQuote, DataError> {
        let xml = self
            .http
            .get(&self.url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_bce_eurofxref_xml(&xml)
    }
}

pub fn parse_bce_eurofxref_xml(xml: &str) -> Result<BceFxQuote, DataError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut current_date: Option<NaiveDate> = None;
    let mut usd_rate: Option<Decimal> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Empty(e) | Event::Start(e)) if e.name().as_ref().ends_with(b"Cube") => {
                let mut time_attr: Option<String> = None;
                let mut currency_attr: Option<String> = None;
                let mut rate_attr: Option<String> = None;

                for attr in e.attributes().flatten() {
                    let key = attr.key.as_ref();
                    let val = String::from_utf8_lossy(&attr.value).into_owned();
                    if key == b"time" {
                        time_attr = Some(val);
                    } else if key == b"currency" {
                        currency_attr = Some(val);
                    } else if key == b"rate" {
                        rate_attr = Some(val);
                    }
                }

                if let Some(t) = time_attr {
                    current_date = NaiveDate::parse_from_str(&t, "%Y-%m-%d").ok();
                }
                if currency_attr.as_deref() == Some("USD") {
                    if let Some(r_str) = rate_attr {
                        if let Ok(r) = Decimal::from_str(&r_str) {
                            usd_rate = Some(r);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DataError::Parse {
                    provider: "BCE_EUROFXREF".to_string(),
                    details: e.to_string(),
                });
            }
            _ => {}
        }
    }

    match (current_date, usd_rate) {
        (Some(date), Some(eur_usd)) => Ok(BceFxQuote { date, eur_usd }),
        _ => Err(DataError::Parse {
            provider: "BCE_EUROFXREF".to_string(),
            details: "Tag Cube con data o cambio USD non trovato nel feed BCE".to_string(),
        }),
    }
}