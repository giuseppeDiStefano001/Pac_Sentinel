use chrono::{DateTime, NaiveDate};
use chrono_tz::{Europe::Rome, Tz};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{de, Deserialize, Deserializer, Serialize};
use std::fmt;

fn deserialize_rome_datetime<'de, D>(deserializer: D) -> Result<DateTime<Tz>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let parsed = DateTime::parse_from_rfc3339(&raw).map_err(de::Error::custom)?;
    Ok(parsed.with_timezone(&Rome))
}

/// Identificativo univoco degli strumenti in portafoglio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AssetId {
    /// iShares Core S&P 500 UCITS ETF Acc (CSSPX.MI / SXR8.DE)
    EtfSp500,
    /// Nvidia Corp. (NVDA)
    Nvda,
    /// MP Materials Corp. (MP)
    Mp,
}

impl AssetId {
    pub const ALL: [AssetId; 3] = [AssetId::EtfSp500, AssetId::Nvda, AssetId::Mp];

    pub fn code(self) -> &'static str {
        match self {
            AssetId::EtfSp500 => "CSSPX.MI",
            AssetId::Nvda => "NVDA",
            AssetId::Mp => "MP",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AssetId::EtfSp500 => "iShares Core S&P 500 UCITS ETF Acc",
            AssetId::Nvda => "Nvidia (NVDA)",
            AssetId::Mp => "MP Materials (MP)",
        }
    }

    pub fn is_satellite(self) -> bool {
        matches!(self, AssetId::Nvda | AssetId::Mp)
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.display_name(), self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Currency {
    Eur,
    Usd,
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Currency::Eur => write!(f, "EUR"),
            Currency::Usd => write!(f, "USD"),
        }
    }
}

/// Livello di allerta del semaforo dei segnali.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SignalLevel {
    Verde,
    Giallo,
    Arancione,
    Rosso,
}

impl fmt::Display for SignalLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalLevel::Verde => write!(f, "VERDE"),
            SignalLevel::Giallo => write!(f, "GIALLO"),
            SignalLevel::Arancione => write!(f, "ARANCIONE"),
            SignalLevel::Rosso => write!(f, "ROSSO"),
        }
    }
}

/// Operazione di acquisto eseguita sul broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuyOperation {
    pub id: Option<i64>,
    pub asset: AssetId,
    pub date: NaiveDate,
    /// Importo speso in EUR (commissioni 0 € su PAC Trade Republic).
    pub invested_eur: Decimal,
    /// Prezzo unitario nella valuta nativa dello strumento al momento dell'acquisto.
    pub unit_price_native: Decimal,
    pub native_currency: Currency,
    /// Cambio EUR/USD usato per la conversione (1.0 se strumento in EUR).
    pub eur_usd_rate: Decimal,
    /// Quote (anche frazionarie) acquistate.
    pub quantity: Decimal,
    pub note: Option<String>,
}

/// Dato di mercato corrente e storico per uno strumento.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub asset: AssetId,
    pub price_native: Decimal,
    pub currency: Currency,
    /// Prezzo convertito in EUR tramite tasso BCE di riferimento.
    pub price_eur: Decimal,
    /// Variazione percentuale a 7 giorni (es. -3.5 = -3,5%).
    pub change_7d_pct: Decimal,
    /// Variazione percentuale a 30 giorni (es. -12.0 = -12,0%).
    pub change_30d_pct: Decimal,
    /// Massimo a 52 settimane nella valuta nativa.
    pub high_52w_native: Decimal,
    pub source: String,
    #[serde(deserialize_with = "deserialize_rome_datetime")]
    pub timestamp_rome: DateTime<Tz>,
    pub is_stale: bool,
}

impl MarketSnapshot {
    /// Drawdown percentuale positivo dal massimo a 52 settimane (0% se ai massimi).
    pub fn drawdown_52w_pct(&self) -> Decimal {
        if self.high_52w_native <= dec!(0) || self.price_native >= self.high_52w_native {
            return dec!(0);
        }
        ((self.high_52w_native - self.price_native) / self.high_52w_native * dec!(100)).round_dp(2)
    }
}

/// Stato del fondo emergenze (strettamente isolato dal portafoglio investimenti).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmergencyFundStatus {
    pub balance_eur: Decimal,
    pub stamp_duty_warning_threshold_eur: Decimal,
    pub stamp_duty_legal_threshold_eur: Decimal,
    pub is_above_warning_threshold: bool,
}