pub mod migrations;

use crate::error::DataError;
use crate::market_hours::is_price_stale;
use chrono::{DateTime, NaiveDate};
use chrono_tz::{Europe::Rome, Tz};
use pac_core::types::{AssetId, BuyOperation, Currency, MarketSnapshot, SignalLevel};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct DailyBar {
    pub date: NaiveDate,
    pub close_native: Decimal,
    pub currency: Currency,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct LatestQuoteRecord {
    pub symbol: String,
    pub price_native: Decimal,
    pub currency: Currency,
    pub source: String,
    pub timestamp_rome: DateTime<Tz>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSignalState {
    pub level: SignalLevel,
    pub last_notified_at_rome: DateTime<Tz>,
}

#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub id: i64,
    pub entry_date: NaiveDate,
    pub asset: Option<AssetId>,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ThesisState {
    pub nvda_confirmed: bool,
    pub nvda_notes: String,
    pub mp_confirmed: bool,
    pub mp_compromised: bool,
    pub mp_notes: String,
}

impl Default for ThesisState {
    fn default() -> Self {
        Self {
            nvda_confirmed: true,
            nvda_notes: "Domanda data center AI e margini coerenti con la tesi di lungo periodo."
                .to_string(),
            mp_confirmed: true,
            mp_compromised: false,
            mp_notes: "Filiera occidentale terre rare e magneti NdFeB in sviluppo; posizione satellite speculativa."
                .to_string(),
        }
    }
}

pub struct PortfolioRepository {
    conn: Connection,
}

impl PortfolioRepository {
    pub fn open_file(path: &Path) -> Result<Self, DataError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DataError::Parse {
                provider: "filesystem".to_string(),
                details: e.to_string(),
            })?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000; PRAGMA synchronous = NORMAL;")?;
        migrations::run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self, DataError> {
        let conn = Connection::open_in_memory()?;
        migrations::run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64, DataError> {
        let v: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    pub fn insert_operation(
        &self,
        op: &BuyOperation,
        is_extra_advance: bool,
    ) -> Result<i64, DataError> {
        self.conn.execute(
            "INSERT INTO operations (asset, op_date, invested_eur, unit_price_native, native_currency, eur_usd_rate, quantity, is_extra_advance, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                asset_to_str(op.asset),
                op.date.to_string(),
                op.invested_eur.to_string(),
                op.unit_price_native.to_string(),
                currency_to_str(op.native_currency),
                op.eur_usd_rate.to_string(),
                op.quantity.to_string(),
                i32::from(is_extra_advance),
                op.note,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_operation(&self, id: i64) -> Result<(), DataError> {
        self.conn
            .execute("DELETE FROM operations WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_operations(&self) -> Result<Vec<BuyOperation>, DataError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, asset, op_date, invested_eur, unit_price_native, native_currency, eur_usd_rate, quantity, note
             FROM operations ORDER BY op_date ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;

        let mut out = Vec::new();
        for item in rows {
            let (id, asset_s, date_s, inv_s, price_s, curr_s, fx_s, qty_s, note) = item?;
            out.push(BuyOperation {
                id: Some(id),
                asset: parse_asset(&asset_s)?,
                date: parse_date(&date_s)?,
                invested_eur: parse_dec(&inv_s)?,
                unit_price_native: parse_dec(&price_s)?,
                native_currency: parse_currency(&curr_s)?,
                eur_usd_rate: parse_dec(&fx_s)?,
                quantity: parse_dec(&qty_s)?,
                note,
            });
        }
        Ok(out)
    }

    pub fn sum_extra_etf_advances_eur(&self) -> Result<Decimal, DataError> {
        let mut stmt = self
            .conn
            .prepare("SELECT invested_eur FROM operations WHERE is_extra_advance = 1")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut total = dec!(0);
        for item in rows {
            total += parse_dec(&item?)?;
        }
        Ok(total)
    }

    pub fn upsert_daily_bars(
        &self,
        symbol: &str,
        bars: &[DailyBar],
        fetched_at: DateTime<Tz>,
    ) -> Result<(), DataError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO price_history (symbol, price_date, close_native, currency, source, fetched_at_rome)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(symbol, price_date) DO UPDATE SET
                    close_native = excluded.close_native,
                    source = excluded.source,
                    fetched_at_rome = excluded.fetched_at_rome",
            )?;
            let ts = fetched_at.to_rfc3339();
            for b in bars {
                stmt.execute(params![
                    symbol,
                    b.date.to_string(),
                    b.close_native.round_dp(4).to_string(),
                    currency_to_str(b.currency),
                    b.source,
                    ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn history_count(&self, symbol: &str) -> Result<u32, DataError> {
        let count: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM price_history WHERE symbol = ?1",
            params![symbol],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    pub fn upsert_latest_quote(&self, q: &LatestQuoteRecord) -> Result<(), DataError> {
        self.conn.execute(
            "INSERT INTO latest_quotes (symbol, price_native, currency, source, timestamp_rome)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(symbol) DO UPDATE SET
                price_native = excluded.price_native,
                currency = excluded.currency,
                source = excluded.source,
                timestamp_rome = excluded.timestamp_rome",
            params![
                q.symbol,
                q.price_native.round_dp(4).to_string(),
                currency_to_str(q.currency),
                q.source,
                q.timestamp_rome.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_latest_quote(&self, symbol: &str) -> Result<Option<LatestQuoteRecord>, DataError> {
        let row_opt = self
            .conn
            .query_row(
                "SELECT symbol, price_native, currency, source, timestamp_rome FROM latest_quotes WHERE symbol = ?1",
                params![symbol],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;

        let Some((s, p, c, src, ts)) = row_opt else {
            return Ok(None);
        };

        Ok(Some(LatestQuoteRecord {
            symbol: s,
            price_native: parse_dec(&p)?,
            currency: parse_currency(&c)?,
            source: src,
            timestamp_rome: parse_rome_ts(&ts)?,
        }))
    }

    pub fn upsert_fx_rate(
        &self,
        date: NaiveDate,
        eur_usd: Decimal,
        source: &str,
    ) -> Result<(), DataError> {
        self.conn.execute(
            "INSERT INTO fx_rates (rate_date, eur_usd, source) VALUES (?1, ?2, ?3)
             ON CONFLICT(rate_date) DO UPDATE SET eur_usd = excluded.eur_usd, source = excluded.source",
            params![date.to_string(), eur_usd.to_string(), source],
        )?;
        Ok(())
    }

    pub fn latest_eur_usd(&self) -> Result<Option<(NaiveDate, Decimal, String)>, DataError> {
        let row_opt = self
            .conn
            .query_row(
                "SELECT rate_date, eur_usd, source FROM fx_rates ORDER BY rate_date DESC LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let Some((d, rate, src)) = row_opt else {
            return Ok(None);
        };
        Ok(Some((parse_date(&d)?, parse_dec(&rate)?, src)))
    }

    pub fn get_signal_state(&self, key: &str) -> Result<Option<PersistedSignalState>, DataError> {
        let row_opt = self
            .conn
            .query_row(
                "SELECT last_level, last_notified_at_rome FROM signal_notification_state WHERE key = ?1",
                params![key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        let Some((lvl_s, ts_s)) = row_opt else {
            return Ok(None);
        };

        Ok(Some(PersistedSignalState {
            level: parse_signal_level(&lvl_s)?,
            last_notified_at_rome: parse_rome_ts(&ts_s)?,
        }))
    }

    pub fn upsert_signal_state(
        &self,
        key: &str,
        level: SignalLevel,
        notified_at: DateTime<Tz>,
    ) -> Result<(), DataError> {
        self.conn.execute(
            "INSERT INTO signal_notification_state (key, last_level, last_notified_at_rome)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                last_level = excluded.last_level,
                last_notified_at_rome = excluded.last_notified_at_rome",
            params![key, signal_level_to_str(level), notified_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn has_dedup_event(&self, event_key: &str) -> Result<bool, DataError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM notification_dedup_events WHERE event_key = ?1",
            params![event_key],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn record_dedup_event(
        &self,
        event_key: &str,
        sent_at: DateTime<Tz>,
    ) -> Result<(), DataError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO notification_dedup_events (event_key, sent_at_rome) VALUES (?1, ?2)",
            params![event_key, sent_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn clear_dedup_event(&self, event_key: &str) -> Result<(), DataError> {
        self.conn.execute(
            "DELETE FROM notification_dedup_events WHERE event_key = ?1",
            params![event_key],
        )?;
        Ok(())
    }

    pub fn set_mute_until(&self, until: Option<DateTime<Tz>>) -> Result<(), DataError> {
        match until {
            Some(dt) => {
                self.conn.execute(
                    "INSERT INTO app_runtime_state (key, value) VALUES ('mute_until_rome', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![dt.to_rfc3339()],
                )?;
            }
            None => {
                self.conn.execute(
                    "DELETE FROM app_runtime_state WHERE key = 'mute_until_rome'",
                    [],
                )?;
            }
        }
        Ok(())
    }

    pub fn get_mute_until(&self) -> Result<Option<DateTime<Tz>>, DataError> {
        let val_opt: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM app_runtime_state WHERE key = 'mute_until_rome'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match val_opt {
            Some(s) => Ok(Some(parse_rome_ts(&s)?)),
            None => Ok(None),
        }
    }

    pub fn set_current_pac_month(&self, month: u32) -> Result<(), DataError> {
        self.conn.execute(
            "INSERT INTO app_runtime_state (key, value) VALUES ('current_pac_month', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![month.to_string()],
        )?;
        Ok(())
    }

    pub fn get_current_pac_month(&self) -> Result<u32, DataError> {
        let val_opt: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM app_runtime_state WHERE key = 'current_pac_month'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(val_opt
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1))
    }

    pub fn set_earnings_date(&self, asset: AssetId, date: NaiveDate) -> Result<(), DataError> {
        self.conn.execute(
            "INSERT INTO earnings_dates (asset, next_earnings_date, reminded_for_date)
             VALUES (?1, ?2, NULL)
             ON CONFLICT(asset) DO UPDATE SET next_earnings_date = excluded.next_earnings_date",
            params![asset_to_str(asset), date.to_string()],
        )?;
        Ok(())
    }

    pub fn list_earnings_dates(&self) -> Result<Vec<(AssetId, NaiveDate)>, DataError> {
        let mut stmt = self
            .conn
            .prepare("SELECT asset, next_earnings_date FROM earnings_dates")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for item in rows {
            let (a, d) = item?;
            out.push((parse_asset(&a)?, parse_date(&d)?));
        }
        Ok(out)
    }

    pub fn load_thesis_state(&self) -> Result<ThesisState, DataError> {
        let mut state = ThesisState::default();
        let mut stmt = self.conn.prepare(
            "SELECT asset, confirmed, compromised, notes_json FROM thesis_checklists",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i32>(1)?,
                r.get::<_, i32>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for item in rows {
            let (asset_s, conf, comp, notes) = item?;
            match asset_s.as_str() {
                "NVDA" => {
                    state.nvda_confirmed = conf != 0;
                    state.nvda_notes = notes;
                }
                "MP" => {
                    state.mp_confirmed = conf != 0;
                    state.mp_compromised = comp != 0;
                    state.mp_notes = notes;
                }
                _ => {}
            }
        }
        Ok(state)
    }

    pub fn save_thesis_state(
        &self,
        state: &ThesisState,
        updated_at: DateTime<Tz>,
    ) -> Result<(), DataError> {
        let ts = updated_at.to_rfc3339();
        self.conn.execute(
            "INSERT INTO thesis_checklists (asset, confirmed, compromised, notes_json, updated_at_rome)
             VALUES ('NVDA', ?1, 0, ?2, ?3)
             ON CONFLICT(asset) DO UPDATE SET
                confirmed = excluded.confirmed,
                notes_json = excluded.notes_json,
                updated_at_rome = excluded.updated_at_rome",
            params![i32::from(state.nvda_confirmed), state.nvda_notes, ts],
        )?;
        self.conn.execute(
            "INSERT INTO thesis_checklists (asset, confirmed, compromised, notes_json, updated_at_rome)
             VALUES ('MP', ?1, ?2, ?3, ?4)
             ON CONFLICT(asset) DO UPDATE SET
                confirmed = excluded.confirmed,
                compromised = excluded.compromised,
                notes_json = excluded.notes_json,
                updated_at_rome = excluded.updated_at_rome",
            params![
                i32::from(state.mp_confirmed),
                i32::from(state.mp_compromised),
                state.mp_notes,
                ts
            ],
        )?;
        Ok(())
    }

    pub fn add_journal_entry(
        &self,
        entry_date: NaiveDate,
        asset: Option<AssetId>,
        title: &str,
        body: &str,
    ) -> Result<i64, DataError> {
        self.conn.execute(
            "INSERT INTO decision_journal (entry_date, asset, title, body) VALUES (?1, ?2, ?3, ?4)",
            params![
                entry_date.to_string(),
                asset.map(asset_to_str),
                title,
                body
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_journal_entries(&self) -> Result<Vec<JournalEntry>, DataError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entry_date, asset, title, body FROM decision_journal ORDER BY entry_date DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for item in rows {
            let (id, d_s, a_opt, title, body) = item?;
            let asset = match a_opt {
                Some(ref s) if !s.is_empty() => Some(parse_asset(s)?),
                _ => None,
            };
            out.push(JournalEntry {
                id,
                entry_date: parse_date(&d_s)?,
                asset,
                title,
                body,
            });
        }
        Ok(out)
    }

    pub fn build_market_snapshot(
        &self,
        asset: AssetId,
        symbol: &str,
        eur_usd_rate: Decimal,
        now_rome: DateTime<Tz>,
        stale_after_hours: i64,
    ) -> Result<Option<MarketSnapshot>, DataError> {
        let Some(latest) = self.get_latest_quote(symbol)? else {
            return Ok(None);
        };

        let mut stmt = self.conn.prepare(
            "SELECT price_date, close_native FROM price_history WHERE symbol = ?1 ORDER BY price_date DESC LIMIT 260",
        )?;
        let rows = stmt.query_map(params![symbol], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;

        let mut closes: Vec<(NaiveDate, Decimal)> = Vec::new();
        for item in rows {
            let (d, c) = item?;
            closes.push((parse_date(&d)?, parse_dec(&c)?));
        }

        let mut high_52w = latest.price_native;
        for (_, close) in closes.iter().take(252) {
            if *close > high_52w {
                high_52w = *close;
            }
        }

        let today = now_rome.date_naive();
        let ref_7d = find_close_on_or_before(&closes, today - chrono::Duration::days(7));
        let ref_30d = find_close_on_or_before(&closes, today - chrono::Duration::days(30));

        let change_7d_pct = pct_change(latest.price_native, ref_7d);
        let change_30d_pct = pct_change(latest.price_native, ref_30d);

        let price_eur = match latest.currency {
            Currency::Eur => latest.price_native,
            Currency::Usd => {
                if eur_usd_rate > dec!(0) {
                    (latest.price_native / eur_usd_rate).round_dp(4)
                } else {
                    dec!(0)
                }
            }
        };

        let is_stale = is_price_stale(asset, latest.timestamp_rome, now_rome, stale_after_hours);

        Ok(Some(MarketSnapshot {
            asset,
            price_native: latest.price_native.round_dp(4),
            currency: latest.currency,
            price_eur,
            change_7d_pct,
            change_30d_pct,
            high_52w_native: high_52w.round_dp(4),
            source: latest.source,
            timestamp_rome: latest.timestamp_rome,
            is_stale,
        }))
    }

    pub fn build_all_snapshots(
        &self,
        etf_symbol: &str,
        eur_usd_rate: Decimal,
        now_rome: DateTime<Tz>,
        stale_after_hours: i64,
    ) -> Result<BTreeMap<AssetId, MarketSnapshot>, DataError> {
        let mut map = BTreeMap::new();
        for asset in AssetId::ALL {
            let sym = if asset == AssetId::EtfSp500 {
                etf_symbol
            } else {
                asset.code()
            };
            if let Some(snap) =
                self.build_market_snapshot(asset, sym, eur_usd_rate, now_rome, stale_after_hours)?
            {
                map.insert(asset, snap);
            }
        }
        Ok(map)
    }
}

fn find_close_on_or_before(closes: &[(NaiveDate, Decimal)], target: NaiveDate) -> Option<Decimal> {
    closes
        .iter()
        .find(|(d, _)| *d <= target)
        .map(|(_, c)| *c)
}

fn pct_change(current: Decimal, reference: Option<Decimal>) -> Decimal {
    match reference {
        Some(r) if r > dec!(0) => ((current - r) / r * dec!(100)).round_dp(2),
        _ => dec!(0),
    }
}

fn asset_to_str(a: AssetId) -> &'static str {
    match a {
        AssetId::EtfSp500 => "ETF_SP500",
        AssetId::Nvda => "NVDA",
        AssetId::Mp => "MP",
    }
}

fn parse_asset(s: &str) -> Result<AssetId, DataError> {
    match s {
        "ETF_SP500" => Ok(AssetId::EtfSp500),
        "NVDA" => Ok(AssetId::Nvda),
        "MP" => Ok(AssetId::Mp),
        _ => Err(DataError::Parse {
            provider: "sqlite".to_string(),
            details: format!("Asset sconosciuto: {s}"),
        }),
    }
}

fn signal_level_to_str(lvl: SignalLevel) -> &'static str {
    match lvl {
        SignalLevel::Verde => "VERDE",
        SignalLevel::Giallo => "GIALLO",
        SignalLevel::Arancione => "ARANCIONE",
        SignalLevel::Rosso => "ROSSO",
    }
}

fn parse_signal_level(s: &str) -> Result<SignalLevel, DataError> {
    match s {
        "VERDE" => Ok(SignalLevel::Verde),
        "GIALLO" => Ok(SignalLevel::Giallo),
        "ARANCIONE" => Ok(SignalLevel::Arancione),
        "ROSSO" => Ok(SignalLevel::Rosso),
        _ => Err(DataError::Parse {
            provider: "sqlite".to_string(),
            details: format!("Livello segnale sconosciuto: {s}"),
        }),
    }
}

fn currency_to_str(c: Currency) -> &'static str {
    match c {
        Currency::Eur => "EUR",
        Currency::Usd => "USD",
    }
}

fn parse_currency(s: &str) -> Result<Currency, DataError> {
    match s {
        "EUR" => Ok(Currency::Eur),
        "USD" => Ok(Currency::Usd),
        _ => Err(DataError::Parse {
            provider: "sqlite".to_string(),
            details: format!("Valuta sconosciuta: {s}"),
        }),
    }
}

fn parse_date(s: &str) -> Result<NaiveDate, DataError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| DataError::Parse {
        provider: "sqlite".to_string(),
        details: e.to_string(),
    })
}

fn parse_rome_ts(s: &str) -> Result<DateTime<Tz>, DataError> {
    DateTime::parse_from_rfc3339(s)
        .map_err(|e| DataError::Parse {
            provider: "sqlite".to_string(),
            details: e.to_string(),
        })
        .map(|dt| dt.with_timezone(&Rome))
}

fn parse_dec(s: &str) -> Result<Decimal, DataError> {
    Decimal::from_str(s).map_err(|e| DataError::Parse {
        provider: "sqlite".to_string(),
        details: e.to_string(),
    })
}