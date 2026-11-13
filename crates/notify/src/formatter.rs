use pac_core::pac::PacAllocation;
use pac_core::portfolio::PortfolioValuation;
use pac_core::signals::{AssetSignalEvaluation, PortfolioSignalsReport};
use pac_core::types::{AssetId, MarketSnapshot, SignalLevel};
use pac_core::LEGAL_DISCLAIMER;
use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// Esegue l'escape rigoroso per `parse_mode = "HTML"` delle API Bot di Telegram.
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn level_badge(level: SignalLevel) -> &'static str {
    match level {
        SignalLevel::Verde => "[VERDE]",
        SignalLevel::Giallo => "[GIALLO]",
        SignalLevel::Arancione => "[ARANCIONE]",
        SignalLevel::Rosso => "[ROSSO]",
    }
}

pub fn format_welcome_message() -> String {
    format!(
        "<b>PAC Sentinel collegato con successo</b>\n\n\
         Questo bot invierà notifiche solo al cambiamento di stato delle regole del portafoglio.\n\
         Comandi disponibili (sola lettura): <code>/stato</code>, <code>/prezzi</code>, <code>/segnali</code>, <code>/pac</code>, <code>/silenzio 24h</code>, <code>/aiuto</code>.\n\n\
         <i>{}</i>",
        escape_html(LEGAL_DISCLAIMER)
    )
}

/// Formatta la notifica di segnale per un singolo strumento con tutti i campi richiesti.
pub fn format_asset_signal_notification(
    eval: &AssetSignalEvaluation,
    previous_level: Option<SignalLevel>,
    snapshot: Option<&MarketSnapshot>,
    emergency_fund_eur: Decimal,
) -> String {
    let prev_str = previous_level.map_or_else(
        || "Iniziale".to_string(),
        |p| escape_html(&p.to_string()),
    );
    let badge = escape_html(level_badge(eval.level));
    let asset_name = escape_html(eval.asset.display_name());
    let code = escape_html(eval.asset.code());

    let market_line = match snapshot {
        Some(s) => {
            let ts = s.timestamp_rome.format("%Y-%m-%d %H:%M %Z");
            format!(
                "Prezzo: <b>{:.2} {}</b> (~{:.2} EUR)\n\
                 Var 7g: <b>{:+.2}%</b> | Var 30g: <b>{:+.2}%</b> | DD 52w: <b>{:.2}%</b>\n\
                 Ultimo aggiornamento: {} ({})",
                s.price_native,
                escape_html(&s.currency.to_string()),
                s.price_eur,
                s.change_7d_pct,
                s.change_30d_pct,
                s.drawdown_52w_pct(),
                escape_html(&ts.to_string()),
                escape_html(&s.source)
            )
        }
        None => "Prezzo: dato non disponibile".to_string(),
    };

    format!(
        "<b>{badge} Segnale {asset_name} ({code})</b>\n\
         Transizione livello: {prev_str} -&gt; <b>{}</b>\n\n\
         {market_line}\n\n\
         <b>Regola scattata:</b> {}\n\
         <b>Azione PAC suggerita:</b> {}\n\n\
         <i>Promemoria: il fondo emergenze ({emergency_fund_eur:.2} EUR) non si usa mai.</i>\n\
         <i>{}</i>",
        escape_html(&eval.level.to_string()),
        escape_html(&eval.triggered_rule),
        escape_html(&eval.suggested_pac_action),
        escape_html(LEGAL_DISCLAIMER)
    )
}

pub fn format_pac_day_reminder(
    alloc: &PacAllocation,
    phase_desc: &str,
    remaining_tr_cash: Decimal,
    emergency_fund_eur: Decimal,
) -> String {
    format!(
        "<b>Promemoria Giorno PAC (Mese {})</b>\n\
         Fase attiva: {}\n\n\
         <b>Ripartizione rata ({:.2} EUR):</b>\n\
         • ETF S&amp;P 500: <b>{:.2} EUR</b>\n\
         • Nvidia (NVDA): <b>{:.2} EUR</b>\n\
         • MP Materials (MP): <b>{:.2} EUR</b>\n\n\
         Liquidità Trade Republic residua: {:.2} EUR\n\
         <i>Promemoria: il fondo emergenze ({emergency_fund_eur:.2} EUR) non si usa mai.</i>\n\
         <i>{}</i>",
        alloc.month_number,
        escape_html(phase_desc),
        alloc.total_eur,
        alloc.etf_sp500_eur,
        alloc.nvda_eur,
        alloc.mp_eur,
        remaining_tr_cash,
        escape_html(LEGAL_DISCLAIMER)
    )
}

pub fn format_custom_alert(title: &str, body: &str, emergency_fund_eur: Decimal) -> String {
    format!(
        "<b>{}</b>\n\n{}\n\n\
         <i>Promemoria: il fondo emergenze ({emergency_fund_eur:.2} EUR) non si usa mai.</i>\n\
         <i>{}</i>",
        escape_html(title),
        escape_html(body),
        escape_html(LEGAL_DISCLAIMER)
    )
}

pub fn format_status_command_reply(
    valuation: &PortfolioValuation,
    report: &PortfolioSignalsReport,
    remaining_tr_cash: Decimal,
    covered_months: u32,
) -> String {
    let mut lines = vec![
        format!(
            "<b>Stato Portafoglio</b> | Semaforo generale: <b>{}</b>",
            escape_html(&report.overall_level.to_string())
        ),
        format!(
            "Investito: {:.2} EUR | Valore attuale: <b>{:.2} EUR</b>",
            valuation.total_invested_eur, valuation.total_market_value_eur
        ),
        format!(
            "P&amp;L lordo: {:+.2} EUR | P&amp;L netto (post 26%): <b>{:+.2} EUR</b>",
            valuation.total_gross_pnl_eur, valuation.total_net_pnl_eur
        ),
        format!(
            "Liquidità TR PAC: {:.2} EUR ({covered_months} rate Fase 1 coperte)",
            remaining_tr_cash
        ),
        String::new(),
        "<b>Pesi (Reale / Atteso / Target):</b>".to_string(),
    ];

    for asset in AssetId::ALL {
        let p = &valuation.positions[&asset];
        let sig = &report.asset_signals[&asset];
        lines.push(format!(
            "• {} {}: <b>{:.2}%</b> / {:.2}% / {:.2}%",
            escape_html(level_badge(sig.level)),
            escape_html(asset.code()),
            p.actual_weight_pct,
            p.expected_weight_pct,
            p.target_weight_pct
        ));
    }

    lines.push(format!(
        "\nEsposizione effettiva NVDA: {:.2}% | Satelliti: {:.2}%",
        valuation.effective_nvda_exposure_pct, valuation.satellites_weight_pct
    ));
    lines.push(format!("\n<i>{}</i>", escape_html(LEGAL_DISCLAIMER)));
    lines.join("\n")
}

pub fn format_prices_command_reply(
    snapshots: &BTreeMap<AssetId, MarketSnapshot>,
    eur_usd: Option<Decimal>,
) -> String {
    let mut lines = vec!["<b>Quotazioni di Riferimento</b>".to_string()];
    if let Some(fx) = eur_usd {
        lines.push(format!("Cambio BCE EUR/USD: <b>{fx}</b>"));
    }
    lines.push(String::new());

    for asset in AssetId::ALL {
        if let Some(s) = snapshots.get(&asset) {
            let stale = if s.is_stale { " [DATI VECCHI]" } else { "" };
            let ts = s.timestamp_rome.format("%d/%m %H:%M");
            lines.push(format!(
                "• <b>{}</b>: {:.2} {} (~{:.2} EUR) | 7g {:+.2}% | 30g {:+.2}% | DD {:.2}% ({ts}){stale}",
                escape_html(asset.code()),
                s.price_native,
                escape_html(&s.currency.to_string()),
                s.price_eur,
                s.change_7d_pct,
                s.change_30d_pct,
                s.drawdown_52w_pct()
            ));
        } else {
            lines.push(format!("• <b>{}</b>: dato non disponibile", escape_html(asset.code())));
        }
    }

    lines.push(
        "\n<i>Nota: prezzi pubblici di riferimento, possono differire da Trade Republic.</i>"
            .to_string(),
    );
    lines.push(format!("<i>{}</i>", escape_html(LEGAL_DISCLAIMER)));
    lines.join("\n")
}

pub fn format_signals_command_reply(report: &PortfolioSignalsReport) -> String {
    let mut lines = vec![format!(
        "<b>Dettaglio Segnali</b> (Generale: <b>{}</b>)\n",
        escape_html(&report.overall_level.to_string())
    )];

    for asset in AssetId::ALL {
        let s = &report.asset_signals[&asset];
        lines.push(format!(
            "{} <b>{}</b>\nRegola: {}\nAzione: {}\n",
            escape_html(level_badge(s.level)),
            escape_html(asset.code()),
            escape_html(&s.triggered_rule),
            escape_html(&s.suggested_pac_action)
        ));
    }

    for alert in &report.portfolio_alerts {
        lines.push(format!("! {}", escape_html(alert)));
    }

    lines.push(format!("<i>{}</i>", escape_html(LEGAL_DISCLAIMER)));
    lines.join("\n")
}