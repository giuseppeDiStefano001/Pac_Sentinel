use chrono::{DateTime, Datelike, Duration, NaiveTime, Timelike, Weekday};
use chrono_tz::{Europe::Rome, Tz};
use pac_core::types::AssetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketVenue {
    /// Borsa Italiana (09:00 - 17:30 Europe/Rome)
    BorsaItaliana,
    /// NYSE / Nasdaq (15:30 - 22:00 Europe/Rome)
    UsEquity,
}

impl MarketVenue {
    pub fn for_asset(asset: AssetId) -> Self {
        match asset {
            AssetId::EtfSp500 => Self::BorsaItaliana,
            AssetId::Nvda | AssetId::Mp => Self::UsEquity,
        }
    }

    pub fn is_open_at(self, dt_rome: DateTime<Tz>) -> bool {
        if matches!(dt_rome.weekday(), Weekday::Sat | Weekday::Sun) {
            return false;
        }
        let time = dt_rome.time();
        match self {
            Self::BorsaItaliana => {
                let open = NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(NaiveTime::MIN);
                let close = NaiveTime::from_hms_opt(17, 30, 0).unwrap_or(NaiveTime::MIN);
                time >= open && time <= close
            }
            Self::UsEquity => {
                let open = NaiveTime::from_hms_opt(15, 30, 0).unwrap_or(NaiveTime::MIN);
                let close = NaiveTime::from_hms_opt(22, 0, 0).unwrap_or(NaiveTime::MIN);
                time >= open && time <= close
            }
        }
    }
}

/// Verifica se almeno un mercato rilevante per il portafoglio è aperto in questo momento.
pub fn is_any_portfolio_market_open(now_rome: DateTime<Tz>) -> bool {
    MarketVenue::BorsaItaliana.is_open_at(now_rome) || MarketVenue::UsEquity.is_open_at(now_rome)
}

/// Restituisce true se è sabato o domenica su Europe/Rome.
pub fn is_weekend_rome(now_rome: DateTime<Tz>) -> bool {
    matches!(now_rome.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Determina se un dato di prezzo è "vecchio" (stale) in orario di mercato.
/// Nei weekend o a mercati chiusi, un prezzo dell'ultima sessione non viene marcato stale
/// a meno che non superi il weekend stesso (> 72 ore).
pub fn is_price_stale(
    asset: AssetId,
    quote_ts_rome: DateTime<Tz>,
    now_rome: DateTime<Tz>,
    stale_after_hours: i64,
) -> bool {
    let venue = MarketVenue::for_asset(asset);
    let age = now_rome.signed_duration_since(quote_ts_rome);
    if venue.is_open_at(now_rome) {
        age > Duration::hours(stale_after_hours)
    } else {
        age > Duration::hours(72)
    }
}

/// Calcola l'intervallo di attesa suggerito per il daemon di polling:
/// - Weekend: pausa fino alla prossima verifica mattutina
/// - Mercato aperto: ogni `poll_minutes_open` minuti (default 15)
/// - Fuori orario feriale: 1 volta ogni 6 ore
pub fn next_poll_interval_seconds(now_rome: DateTime<Tz>, poll_minutes_open: u64) -> u64 {
    if is_weekend_rome(now_rome) {
        let hours_until_check = if now_rome.hour() < 8 { 2 } else { 6 };
        return hours_until_check * 3600;
    }
    if is_any_portfolio_market_open(now_rome) {
        poll_minutes_open.max(1) * 60
    } else {
        6 * 3600
    }
}

pub fn now_rome() -> DateTime<Tz> {
    chrono::Utc::now().with_timezone(&Rome)
}