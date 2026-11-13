use crate::secrets;
use anyhow::Result;
use chrono::{DateTime, Datelike, Timelike};
use chrono_tz::Tz;
use pac_core::config::PortfolioConfig;
use pac_core::pac::{covered_phase1_months, remaining_tr_liquidity};
use pac_core::portfolio::{evaluate_portfolio, PortfolioValuation};
use pac_core::signals::{
    evaluate_signals, is_phase_transition_month, phase_label, PortfolioSignalsReport,
    QualitativeContext,
};
use pac_core::types::{AssetId, MarketSnapshot, SignalLevel};
use pac_data::db::PortfolioRepository;
use pac_data::market_hours::{next_poll_interval_seconds, now_rome};
use pac_data::providers::bce::BceFxClient;
use pac_data::providers::FallbackPriceChain;
use pac_data::service::MarketDataService;
use pac_notify::client::TelegramBotClient;
use pac_notify::commands::{handle_authorized_command, parse_bot_command};
use pac_notify::formatter::{
    format_asset_signal_notification, format_custom_alert, format_pac_day_reminder,
    format_status_command_reply, format_welcome_message,
};
use pac_notify::policy::NotificationGatekeeper;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

/// Configurazione guidata interattiva per collegare il bot Telegram personale.
pub async fn run_telegram_setup(config_path: &Path, config: &mut PortfolioConfig) -> Result<()> {
    println!("=== CONFIGURAZIONE GUIDATA BOT TELEGRAM ===");
    println!("1. Apri Telegram, cerca @BotFather e crea un nuovo bot con /newbot.");
    println!("2. Incolla qui sotto il token HTTP API ricevuto (verrà salvato solo nel keyring OS):");
    print!("Token Bot: ");
    io::stdout().flush()?;

    let mut token = String::new();
    io::stdin().read_line(&mut token)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("Token vuoto: configurazione annullata.");
    }

    secrets::store_secret("telegram_bot_token", &token)?;
    println!("Token salvato in sicurezza nel portachiavi del sistema operativo.");

    let client = TelegramBotClient::new(token)?;
    println!("\n3. Ora apri la chat con il tuo nuovo bot su Telegram e inviagli /start.");
    println!("In attesa del messaggio /start (timeout 60 secondi)...");

    let mut offset: Option<i64> = None;
    let mut detected_chat_id: Option<i64> = None;

    for _ in 0..12 {
        let updates = client.get_updates(offset, 5).await?;
        for upd in updates {
            offset = Some(upd.update_id + 1);
            if let Some(msg) = upd.message {
                if msg
                    .text
                    .as_deref()
                    .is_some_and(|t| t.trim().starts_with("/start"))
                {
                    detected_chat_id = Some(msg.chat.id);
                    break;
                }
            }
        }
        if detected_chat_id.is_some() {
            break;
        }
    }

    let Some(chat_id) = detected_chat_id else {
        anyhow::bail!("Nessun /start ricevuto entro 60 secondi. Riprova il comando.");
    };

    config.notifications.authorized_chat_id = Some(chat_id);
    std::fs::write(config_path, config.to_toml_string()?)?;

    client
        .send_html_message(chat_id, &format_welcome_message())
        .await?;

    println!("Chat autorizzata rilevata e salvata! Messaggio di prova inviato su Telegram.");
    Ok(())
}

/// Esegue il daemon headless (`app --daemon`): polling mercati + notifiche + comandi Telegram.
pub async fn run_daemon_loop(
    repo: &PortfolioRepository,
    config: &PortfolioConfig,
    chain: &FallbackPriceChain,
) -> Result<()> {
    let token = secrets::load_secret("telegram_bot_token")
        .ok_or_else(|| anyhow::anyhow!("Token Telegram mancante nel keyring. Esegui prima `app telegram-setup`."))?;
    let chat_id = config
        .notifications
        .authorized_chat_id
        .ok_or_else(|| anyhow::anyhow!("Chat ID non configurato. Esegui prima `app telegram-setup`."))?;

    let bot = TelegramBotClient::new(token)?;
    let http = reqwest::Client::new();
    let bce = BceFxClient::new(http);
    let market_service = MarketDataService::new(repo, chain, &bce);

    info!("Avvio daemon PAC Sentinel in modalità background");

    let mut update_offset: Option<i64> = None;
    let mut next_market_poll = tokio::time::Instant::now();

    loop {
        if let Ok(updates) = bot.get_updates(update_offset, 5).await {
            for upd in updates {
                update_offset = Some(upd.update_id + 1);
                let Some(msg) = upd.message else { continue };
                if msg.chat.id != chat_id {
                    continue;
                }
                if let Some(text) = msg.text {
                    let cmd = parse_bot_command(&text);
                    if let Ok(reply_html) =
                        handle_authorized_command(&cmd, repo, config, 1, now_rome())
                    {
                        let _ = bot.send_html_message(chat_id, &reply_html).await;
                    }
                }
            }
        }

        if tokio::time::Instant::now() >= next_market_poll {
            if let Err(err) =
                evaluate_and_notify_cycle(repo, config, &market_service, &bot, chat_id).await
            {
                warn!(error = %err, "Errore durante ciclo di controllo daemon");
            }
            let wait_secs =
                next_poll_interval_seconds(now_rome(), config.market.poll_interval_minutes_market_open);
            next_market_poll = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
        }
    }
}

async fn evaluate_and_notify_cycle(
    repo: &PortfolioRepository,
    config: &PortfolioConfig,
    market_service: &MarketDataService<'_>,
    bot: &TelegramBotClient,
    chat_id: i64,
) -> Result<()> {
    let now = now_rome();
    let snapshots = market_service.refresh_all_market_data(config).await?;
    let operations = repo.list_operations()?;
    let current_month = 1_u32;
    let completed_months = current_month.saturating_sub(1);

    let valuation = evaluate_portfolio(config, &operations, &snapshots, completed_months)?;
    let tr_cash =
        remaining_tr_liquidity(config, completed_months.min(config.pac.phase1_months), dec!(0))?;

    let mut prev_levels = BTreeMap::new();
    for asset in AssetId::ALL {
        let key = format!("asset_signal:{}", asset.code());
        if let Some(st) = repo.get_signal_state(&key)? {
            prev_levels.insert(asset, st.level);
        }
    }

    let ctx = QualitativeContext {
        current_pac_month: current_month,
        remaining_tr_cash_eur: tr_cash,
        evaluation_date: Some(now.date_naive()),
        nvda_thesis_confirmed: true,
        mp_thesis_compromised: false,
        previous_levels: prev_levels,
        ..QualitativeContext::default()
    };
    let report = evaluate_signals(config, &valuation, &snapshots, &ctx)?;
    let gate = NotificationGatekeeper::new(repo, &config.notifications);
    let ef_eur = config.profile.emergency_fund_eur;

    for asset in AssetId::ALL {
        let eval = &report.asset_signals[&asset];
        if let Some(prev_lvl) = gate.should_notify_asset_signal(asset, eval.level, now)? {
            let msg = format_asset_signal_notification(eval, prev_lvl, snapshots.get(&asset), ef_eur);
            bot.send_html_message(chat_id, &msg).await?;
        }
    }

    dispatch_scheduled_reminders(
        repo,
        config,
        &gate,
        bot,
        chat_id,
        &valuation,
        &report,
        &snapshots,
        current_month,
        tr_cash,
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_scheduled_reminders(
    repo: &PortfolioRepository,
    config: &PortfolioConfig,
    gate: &NotificationGatekeeper<'_>,
    bot: &TelegramBotClient,
    chat_id: i64,
    valuation: &PortfolioValuation,
    report: &PortfolioSignalsReport,
    snapshots: &BTreeMap<AssetId, MarketSnapshot>,
    current_month: u32,
    tr_cash: Decimal,
    now: DateTime<Tz>,
) -> Result<()> {
    let ef_eur = config.profile.emergency_fund_eur;
    let covered = covered_phase1_months(config, tr_cash);
    let today = now.date_naive();

    if today.day() == u32::from(config.pac.execution_day_of_month) {
        let key = format!("pac_day:{}-{:02}", today.year(), today.month());
        if gate.try_acquire_dedup_event(&key, false, now)? {
            let msg = format_pac_day_reminder(
                &report.adjusted_pac_allocation,
                phase_label(report.adjusted_pac_allocation.phase),
                tr_cash,
                ef_eur,
            );
            bot.send_html_message(chat_id, &msg).await?;
        }
    }

    if is_phase_transition_month(config, current_month) {
        let key = format!("phase_transition:{current_month}");
        if gate.try_acquire_dedup_event(&key, false, now)? {
            let msg = format_custom_alert(
                "Passaggio PAC da Fase 1 a Fase 2",
                "Dal mese 25 il PAC passa da 250 EUR/mese (liquidità TR) a 100 EUR/mese dai risparmi correnti (80 / 15 / 5 EUR).",
                ef_eur,
            );
            bot.send_html_message(chat_id, &msg).await?;
        }
    }

    if covered < config.notifications.low_cash_warning_remaining_months
        && current_month <= config.pac.phase1_months
    {
        let key = format!("low_tr_cash:{}-{:02}", today.year(), today.month());
        if gate.try_acquire_dedup_event(&key, false, now)? {
            let msg = format_custom_alert(
                "Liquidità Trade Republic PAC in esaurimento",
                &format!(
                    "Restano {tr_cash:.2} EUR su Trade Republic (meno di {} rate di Fase 1).",
                    config.notifications.low_cash_warning_remaining_months
                ),
                ef_eur,
            );
            bot.send_html_message(chat_id, &msg).await?;
        }
    }

    for (asset, earnings_date) in repo.list_earnings_dates()? {
        if today + chrono::Duration::days(1) == earnings_date {
            let key = format!("earnings:{}:{earnings_date}", asset.code());
            if gate.try_acquire_dedup_event(&key, false, now)? {
                let msg = format_custom_alert(
                    &format!("Promemoria Trimestrale {} domani", asset.code()),
                    &format!(
                        "Domani ({earnings_date}) sono attesi i risultati trimestrali di {}. Nessuna azione impulsiva pre-dati.",
                        asset.display_name()
                    ),
                    ef_eur,
                );
                bot.send_html_message(chat_id, &msg).await?;
            }
        }
    }

    for asset in AssetId::ALL {
        let stale_key = format!("stale_data:{}", asset.code());
        if let Some(s) = snapshots.get(&asset) {
            if s.is_stale {
                if gate.try_acquire_dedup_event(&stale_key, false, now)? {
                    let msg = format_custom_alert(
                        &format!("Dati di mercato non aggiornati ({})", asset.code()),
                        "Le quotazioni risultano ferme da oltre la soglia configurata in orario di mercato.",
                        ef_eur,
                    );
                    bot.send_html_message(chat_id, &msg).await?;
                }
            } else {
                repo.clear_dedup_event(&stale_key)?;
            }
        }
    }

    if today.day() == 1 && matches!(today.month(), 1 | 4 | 7 | 10) {
        let key = format!("quarterly_review:{}-Q{}", today.year(), (today.month() - 1) / 3 + 1);
        if gate.try_acquire_dedup_event(&key, false, now)? {
            let status_html = format_status_command_reply(valuation, report, tr_cash, covered);
            bot.send_html_message(
                chat_id,
                &format!("<b>Revisione Trimestrale Portafoglio</b>\n\n{status_html}"),
            )
            .await?;
        }
    }

    if config.notifications.daily_summary_enabled
        && now.hour() == config.notifications.daily_summary_hour_rome
    {
        let key = format!("daily_summary:{today}");
        let is_red = report.overall_level == SignalLevel::Rosso;
        if gate.try_acquire_dedup_event(&key, is_red, now)? {
            let status_html = format_status_command_reply(valuation, report, tr_cash, covered);
            bot.send_html_message(chat_id, &status_html).await?;
        }
    }

    Ok(())
}