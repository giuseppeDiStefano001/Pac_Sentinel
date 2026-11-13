use crate::error::NotifyError;
use crate::formatter::{
    escape_html, format_pac_day_reminder, format_prices_command_reply,
    format_signals_command_reply, format_status_command_reply,
};
use chrono::{DateTime, Duration};
use chrono_tz::Tz;
use pac_core::config::PortfolioConfig;
use pac_core::pac::{covered_phase1_months, remaining_tr_liquidity};
use pac_core::portfolio::evaluate_portfolio;
use pac_core::signals::{evaluate_signals, phase_label, QualitativeContext};
use pac_core::LEGAL_DISCLAIMER;
use pac_data::db::PortfolioRepository;
use rust_decimal_macros::dec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotCommand {
    Stato,
    Prezzi,
    Segnali,
    Pac,
    Silenzio { hours: Option<i64> },
    Aiuto,
    Unknown,
}

pub fn parse_bot_command(raw_text: &str) -> BotCommand {
    let trimmed = raw_text.trim();
    let mut parts = trimmed.split_whitespace();
    let Some(cmd_raw) = parts.next() else {
        return BotCommand::Unknown;
    };
    // Rimuove eventuale suffisso @NomeBot nei gruppi/chat
    let cmd = cmd_raw.split('@').next().unwrap_or(cmd_raw).to_ascii_lowercase();

    match cmd.as_str() {
        "/stato" | "/status" => BotCommand::Stato,
        "/prezzi" | "/prices" => BotCommand::Prezzi,
        "/segnali" | "/signals" => BotCommand::Segnali,
        "/pac" => BotCommand::Pac,
        "/aiuto" | "/help" | "/start" => BotCommand::Aiuto,
        "/silenzio" => {
            let arg = parts.next().unwrap_or("24h").to_ascii_lowercase();
            if arg == "off" || arg == "0" {
                BotCommand::Silenzio { hours: None }
            } else {
                let num_str = arg.trim_end_matches('h');
                let hours = num_str.parse::<i64>().unwrap_or(24).clamp(1, 168);
                BotCommand::Silenzio { hours: Some(hours) }
            }
        }
        _ => BotCommand::Unknown,
    }
}

/// Esegue un comando di sola lettura autorizzato e restituisce la risposta HTML pronta per Telegram.
pub fn handle_authorized_command(
    cmd: &BotCommand,
    repo: &PortfolioRepository,
    config: &PortfolioConfig,
    current_pac_month: u32,
    now_rome: DateTime<Tz>,
) -> Result<String, NotifyError> {
    match cmd {
        BotCommand::Aiuto => Ok(format!(
            "<b>Comandi disponibili (sola lettura):</b>\n\
             • <code>/stato</code> - Riepilogo valore, P&amp;L, pesi reali vs attesi\n\
             • <code>/prezzi</code> - Ultime quotazioni di riferimento e cambio BCE\n\
             • <code>/segnali</code> - Semaforo regole per ETF, NVDA e MP\n\
             • <code>/pac</code> - Prossima rata, fase attiva e liquidità TR\n\
             • <code>/silenzio 24h</code> - Sospende le notifiche (usa <code>/silenzio off</code> per riattivare)\n\
             • <code>/aiuto</code> - Mostra questo elenco\n\n\
             <i>{}</i>",
            escape_html(LEGAL_DISCLAIMER)
        )),

        BotCommand::Silenzio { hours } => {
            if let Some(h) = hours {
                let until = now_rome + Duration::hours(*h);
                repo.set_mute_until(Some(until))?;
                let until_str = until.format("%Y-%m-%d %H:%M %Z");
                Ok(format!(
                    "Notifiche silenziose per <b>{h} ore</b> (fino a {}).\n\
                     I comandi manuali restano attivi. Usa <code>/silenzio off</code> per riattivare prima.",
                    escape_html(&until_str.to_string())
                ))
            } else {
                repo.set_mute_until(None)?;
                Ok("Notifiche Telegram riattivate.".to_string())
            }
        }

        BotCommand::Prezzi => {
            let eur_usd = repo.latest_eur_usd()?.map(|(_, r, _)| r);
            let fx_val = eur_usd.unwrap_or(dec!(1));
            let snapshots = repo.build_all_snapshots(
                &config.market.etf_symbol,
                fx_val,
                now_rome,
                config.market.stale_after_hours,
            )?;
            Ok(format_prices_command_reply(&snapshots, eur_usd))
        }

        BotCommand::Stato | BotCommand::Segnali | BotCommand::Pac => {
            let fx_val = repo
                .latest_eur_usd()?
                .map_or(dec!(1), |(_, r, _)| r);
            let snapshots = repo.build_all_snapshots(
                &config.market.etf_symbol,
                fx_val,
                now_rome,
                config.market.stale_after_hours,
            )?;
            let operations = repo.list_operations()?;
            let completed_months = current_pac_month.saturating_sub(1);
            let valuation = evaluate_portfolio(config, &operations, &snapshots, completed_months)?;
            let tr_cash = remaining_tr_liquidity(config, completed_months.min(config.pac.phase1_months), dec!(0))?;
            let covered = covered_phase1_months(config, tr_cash);

            let ctx = QualitativeContext {
                current_pac_month: current_pac_month.max(1),
                remaining_tr_cash_eur: tr_cash,
                evaluation_date: Some(now_rome.date_naive()),
                nvda_thesis_confirmed: true,
                mp_thesis_compromised: false,
                ..QualitativeContext::default()
            };
            let report = evaluate_signals(config, &valuation, &snapshots, &ctx)?;

            match cmd {
                BotCommand::Stato => Ok(format_status_command_reply(
                    &valuation,
                    &report,
                    tr_cash,
                    covered,
                )),
                BotCommand::Segnali => Ok(format_signals_command_reply(&report)),
                BotCommand::Pac => Ok(format_pac_day_reminder(
                    &report.adjusted_pac_allocation,
                    phase_label(report.adjusted_pac_allocation.phase),
                    tr_cash,
                    config.profile.emergency_fund_eur,
                )),
                _ => unreachable!(),
            }
        }

        BotCommand::Unknown => Ok(
            "Comando non riconosciuto. Usa <code>/aiuto</code> per l'elenco dei comandi di sola lettura."
                .to_string(),
        ),
    }
}