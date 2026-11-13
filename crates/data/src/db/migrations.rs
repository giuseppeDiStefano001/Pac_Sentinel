use crate::error::DataError;
use rusqlite::Connection;
use tracing::info;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_schema",
        sql: r"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS operations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                asset TEXT NOT NULL,
                op_date TEXT NOT NULL,
                invested_eur TEXT NOT NULL,
                unit_price_native TEXT NOT NULL,
                native_currency TEXT NOT NULL,
                eur_usd_rate TEXT NOT NULL,
                quantity TEXT NOT NULL,
                is_extra_advance INTEGER NOT NULL DEFAULT 0,
                note TEXT
            );

            CREATE TABLE IF NOT EXISTS price_history (
                symbol TEXT NOT NULL,
                price_date TEXT NOT NULL,
                close_native TEXT NOT NULL,
                currency TEXT NOT NULL,
                source TEXT NOT NULL,
                fetched_at_rome TEXT NOT NULL,
                PRIMARY KEY (symbol, price_date)
            );

            CREATE TABLE IF NOT EXISTS latest_quotes (
                symbol TEXT PRIMARY KEY,
                price_native TEXT NOT NULL,
                currency TEXT NOT NULL,
                source TEXT NOT NULL,
                timestamp_rome TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS fx_rates (
                rate_date TEXT PRIMARY KEY,
                eur_usd TEXT NOT NULL,
                source TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS signal_notification_state (
                key TEXT PRIMARY KEY,
                last_level TEXT NOT NULL,
                last_notified_at_rome TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS decision_journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_date TEXT NOT NULL,
                asset TEXT,
                title TEXT NOT NULL,
                body TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS thesis_checklists (
                asset TEXT PRIMARY KEY,
                confirmed INTEGER NOT NULL,
                compromised INTEGER NOT NULL DEFAULT 0,
                notes_json TEXT NOT NULL,
                updated_at_rome TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS earnings_dates (
                asset TEXT PRIMARY KEY,
                next_earnings_date TEXT NOT NULL,
                reminded_for_date TEXT
            );
        ",
    },
    Migration {
        version: 2,
        name: "notification_dedup_and_mute",
        sql: r"
            CREATE TABLE IF NOT EXISTS app_runtime_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS notification_dedup_events (
                event_key TEXT PRIMARY KEY,
                sent_at_rome TEXT NOT NULL
            );
        ",
    },
];

pub fn run_migrations(conn: &Connection) -> Result<(), DataError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    for m in MIGRATIONS {
        if m.version > current_version {
            info!(version = m.version, name = m.name, "Applicazione migrazione DB");
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(m.sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![m.version, m.name, chrono::Utc::now().to_rfc3339()],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}