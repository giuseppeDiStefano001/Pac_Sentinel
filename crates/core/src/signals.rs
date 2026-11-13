use crate::config::PortfolioConfig;
use crate::pac::{
    apply_flow_redirection, calculate_base_pac_allocation, covered_phase1_months,
    FlowRedirectDirective, PacAllocation, PacPhase,
};
use crate::portfolio::PortfolioValuation;
use crate::types::{AssetId, MarketSnapshot, SignalLevel};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Contesto qualitativo e operativo mantenuto dall'utente nel DB / GUI.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QualitativeContext {
    /// Numero del mese corrente del PAC (1-based).
    pub current_pac_month: u32,
    /// Liquidità residua effettiva su Trade Republic (EUR).
    pub remaining_tr_cash_eur: Decimal,
    /// Quota percentuale della liquidità iniziale già usata in anticipi straordinari sull'ETF.
    pub cumulative_etf_advance_used_pct: Decimal,
    /// Data dell'ultimo anticipo straordinario eseguito sull'ETF (per rispettare i 14 giorni tra tranche).
    pub last_etf_advance_date: Option<NaiveDate>,
    /// Data odierna di valutazione.
    pub evaluation_date: Option<NaiveDate>,
    /// Esito della checklist di tesi su NVDA (true = tesi ancora valida).
    pub nvda_thesis_confirmed: bool,
    /// Flag manuale "tesi compromessa" su MP Materials.
    pub mp_thesis_compromised: bool,
    /// Livelli precedenti dei segnali per applicare l'isteresi ed evitare oscillazioni di notifica.
    pub previous_levels: BTreeMap<AssetId, SignalLevel>,
}

/// Risultato della valutazione dei segnali per un singolo strumento.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetSignalEvaluation {
    pub asset: AssetId,
    pub level: SignalLevel,
    /// Descrizione esplicita della regola che ha determinato il livello attuale.
    pub triggered_rule: String,
    /// Azione operativa suggerita sul PAC (mai ordini impulsivi, mai uso del fondo emergenze).
    pub suggested_pac_action: String,
    /// Eventuale importo massimo di anticipo straordinario suggerito sull'ETF (solo Fase 1).
    pub suggested_etf_advance_eur: Option<Decimal>,
    /// Mesi di PAC Fase 1 che resterebbero coperti dopo l'eventuale anticipo.
    pub covered_months_after_advance: Option<u32>,
    /// Richiesta di revisione della checklist fondamentale.
    pub checklist_review_required: bool,
}

/// Esito completo del motore dei segnali sull'intero portafoglio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortfolioSignalsReport {
    pub asset_signals: BTreeMap<AssetId, AssetSignalEvaluation>,
    pub portfolio_alerts: Vec<String>,
    pub overall_level: SignalLevel,
    pub base_pac_allocation: PacAllocation,
    pub adjusted_pac_allocation: PacAllocation,
    pub emergency_fund_reminder: String,
}

fn effective_threshold_with_hysteresis(
    base_threshold: Decimal,
    previous_level: Option<SignalLevel>,
    target_level: SignalLevel,
    hysteresis: Decimal,
) -> Decimal {
    match previous_level {
        Some(prev) if prev >= target_level => (base_threshold - hysteresis).max(dec!(0)),
        _ => base_threshold,
    }
}

/// Valuta tutte le regole del motore dei segnali (funzione pura).
pub fn evaluate_signals(
    config: &PortfolioConfig,
    valuation: &PortfolioValuation,
    snapshots: &BTreeMap<AssetId, MarketSnapshot>,
    ctx: &QualitativeContext,
) -> Result<PortfolioSignalsReport, crate::error::CoreError> {
    let month = ctx.current_pac_month.max(1);
    let base_pac = calculate_base_pac_allocation(config, month)?;
    let mut directive = FlowRedirectDirective::default();
    let mut asset_signals = BTreeMap::new();

    asset_signals.insert(
        AssetId::EtfSp500,
        evaluate_etf(config, snapshots.get(&AssetId::EtfSp500), ctx),
    );
    asset_signals.insert(
        AssetId::Nvda,
        evaluate_nvda(
            config,
            valuation,
            snapshots.get(&AssetId::Nvda),
            ctx,
            &mut directive,
        ),
    );
    asset_signals.insert(
        AssetId::Mp,
        evaluate_mp(
            config,
            valuation,
            snapshots.get(&AssetId::Mp),
            ctx,
            &mut directive,
        ),
    );

    let mut portfolio_alerts = Vec::new();
    if valuation.effective_nvda_exposure_pct > config.thresholds.portfolio_nvda_effective_max_pct
    {
        let eff = valuation.effective_nvda_exposure_pct;
        let inside = config.weights.nvda_inside_etf_pct;
        let limit = config.thresholds.portfolio_nvda_effective_max_pct;
        portfolio_alerts.push(format!(
            "AVVISO PORTAFOGLIO: Esposizione effettiva a NVDA al {eff:.2}% (diretta + ~{inside}% nell'ETF), oltre il limite del {limit}%."
        ));
    }

    if valuation.satellites_weight_pct > config.thresholds.portfolio_satellites_max_pct {
        let sat = valuation.satellites_weight_pct;
        let limit = config.thresholds.portfolio_satellites_max_pct;
        portfolio_alerts.push(format!(
            "AVVISO PORTAFOGLIO: Peso complessivo dei satelliti (NVDA + MP) al {sat:.2}%, oltre il limite del {limit}%."
        ));
    }

    let adjusted_pac = apply_flow_redirection(base_pac, directive);

    let mut overall_level = asset_signals
        .values()
        .map(|s| s.level)
        .max()
        .unwrap_or(SignalLevel::Verde);

    if !portfolio_alerts.is_empty() && overall_level == SignalLevel::Verde {
        overall_level = SignalLevel::Giallo;
    }

    let ef = config.profile.emergency_fund_eur;
    Ok(PortfolioSignalsReport {
        asset_signals,
        portfolio_alerts,
        overall_level,
        base_pac_allocation: base_pac,
        adjusted_pac_allocation: adjusted_pac,
        emergency_fund_reminder: format!(
            "Promemoria: il fondo emergenze ({ef:.2} €) è intoccabile e non viene mai usato per anticipi o acquisti."
        ),
    })
}

fn build_etf_drawdown_eval(
    config: &PortfolioConfig,
    ctx: &QualitativeContext,
    level: SignalLevel,
    triggered_rule: String,
    cap_pct: Decimal,
) -> AssetSignalEvaluation {
    let is_phase1 = ctx.current_pac_month.max(1) <= config.pac.phase1_months;
    let (advance_eur, covered_after, action) =
        compute_etf_advance_suggestion(config, ctx, cap_pct, is_phase1);
    AssetSignalEvaluation {
        asset: AssetId::EtfSp500,
        level,
        triggered_rule,
        suggested_pac_action: action,
        suggested_etf_advance_eur: advance_eur,
        covered_months_after_advance: covered_after,
        checklist_review_required: false,
    }
}

fn evaluate_etf(
    config: &PortfolioConfig,
    snap_opt: Option<&MarketSnapshot>,
    ctx: &QualitativeContext,
) -> AssetSignalEvaluation {
    let t = &config.thresholds;
    let prev = ctx.previous_levels.get(&AssetId::EtfSp500).copied();

    let Some(snap) = snap_opt else {
        return AssetSignalEvaluation {
            asset: AssetId::EtfSp500,
            level: SignalLevel::Giallo,
            triggered_rule: "Dato di mercato ETF non disponibile".to_string(),
            suggested_pac_action: "Mantieni il PAC ordinario previsto.".to_string(),
            suggested_etf_advance_eur: None,
            covered_months_after_advance: None,
            checklist_review_required: false,
        };
    };

    let dd = snap.drawdown_52w_pct();
    let var30 = snap.change_30d_pct;

    let dd_red = effective_threshold_with_hysteresis(
        t.etf_dd_red_min_pct,
        prev,
        SignalLevel::Rosso,
        t.hysteresis_pct,
    );
    let dd_orange = effective_threshold_with_hysteresis(
        t.etf_dd_orange_min_pct,
        prev,
        SignalLevel::Arancione,
        t.hysteresis_pct,
    );
    let dd_yellow = effective_threshold_with_hysteresis(
        t.etf_dd_yellow_min_pct,
        prev,
        SignalLevel::Giallo,
        t.hysteresis_pct,
    );

    if dd > dd_red {
        let min_red = t.etf_dd_red_min_pct;
        return build_etf_drawdown_eval(
            config,
            ctx,
            SignalLevel::Rosso,
            format!("ETF Drawdown 52w al {dd:.2}% (> {min_red}%): fascia ROSSO."),
            t.etf_advance_red_cum_max_pct,
        );
    }

    if dd >= dd_orange {
        return build_etf_drawdown_eval(
            config,
            ctx,
            SignalLevel::Arancione,
            format!("ETF Drawdown 52w al {dd:.2}% (fascia 20-30%): fascia ARANCIONE."),
            t.etf_advance_orange_cum_max_pct,
        );
    }

    if dd >= dd_yellow {
        return build_etf_drawdown_eval(
            config,
            ctx,
            SignalLevel::Giallo,
            format!("ETF Drawdown 52w al {dd:.2}% (fascia 10-20%): fascia GIALLO."),
            t.etf_advance_yellow_max_pct,
        );
    }

    let band = t.etf_var30_normal_band_pct;
    let min_y = t.etf_dd_yellow_min_pct;
    let rule = if var30.abs() <= band {
        format!("ETF variazione 30g ({var30:+.2}%) entro ±{band}% e drawdown ({dd:.2}%) < {min_y}%.")
    } else {
        format!("ETF variazione 30g ({var30:+.2}%) ma drawdown ({dd:.2}%) < {min_y}%: nessuna azione sui rialzi.")
    };

    AssetSignalEvaluation {
        asset: AssetId::EtfSp500,
        level: SignalLevel::Verde,
        triggered_rule: rule,
        suggested_pac_action: "PAC invariato secondo il piano mensile.".to_string(),
        suggested_etf_advance_eur: None,
        covered_months_after_advance: None,
        checklist_review_required: false,
    }
}

fn compute_etf_advance_suggestion(
    config: &PortfolioConfig,
    ctx: &QualitativeContext,
    cumulative_cap_pct: Decimal,
    is_phase1: bool,
) -> (Option<Decimal>, Option<u32>, String) {
    if !is_phase1 || ctx.remaining_tr_cash_eur <= dec!(0) {
        return (
            None,
            Some(0),
            "PAC invariato. In Fase 2 (o con liquidità TR esaurita) non si effettuano anticipi straordinari.".to_string(),
        );
    }

    if let (Some(last_date), Some(today)) = (ctx.last_etf_advance_date, ctx.evaluation_date) {
        let days_elapsed = (today - last_date).num_days();
        let min_days = i64::from(config.thresholds.etf_advance_min_days_between_tranches);
        if days_elapsed < min_days {
            let remaining_days = min_days - days_elapsed;
            let covered = covered_phase1_months(config, ctx.remaining_tr_cash_eur);
            return (
                Some(dec!(0)),
                Some(covered),
                format!(
                    "PAC invariato. Tetto cumulativo {cumulative_cap_pct}%, ma attendere ancora {remaining_days} giorni (minimo {min_days} giorni tra tranche). Rate residue coperte: {covered}."
                ),
            );
        }
    }

    let remaining_allowance_pct =
        (cumulative_cap_pct - ctx.cumulative_etf_advance_used_pct).max(dec!(0));
    let max_advance_eur = (ctx.remaining_tr_cash_eur * remaining_allowance_pct / dec!(100))
        .round_dp(2)
        .min(ctx.remaining_tr_cash_eur);

    let cash_after = (ctx.remaining_tr_cash_eur - max_advance_eur).max(dec!(0));
    let covered_after = covered_phase1_months(config, cash_after);
    let min_days = config.thresholds.etf_advance_min_days_between_tranches;

    let action = if max_advance_eur > dec!(0) {
        format!(
            "PAC invariato. Valuta anticipo sull'ETF fino a {max_advance_eur:.2} € (tetto cumulativo {cumulative_cap_pct}% della liquidità TR residua, tranche distanziate di almeno {min_days} giorni). \
             Dopo l'anticipo resterebbero {cash_after:.2} € su Trade Republic, sufficienti a coprire {covered_after} rate mensili del PAC."
        )
    } else {
        let rem_cash = ctx.remaining_tr_cash_eur;
        format!(
            "PAC invariato. Tetto cumulativo di anticipo ({cumulative_cap_pct}%) già raggiunto; la liquidità residua ({rem_cash:.2} €) prosegue a finanziare {covered_after} rate del PAC."
        )
    };

    (Some(max_advance_eur), Some(covered_after), action)
}

fn evaluate_nvda_market_rules(
    config: &PortfolioConfig,
    snap: &MarketSnapshot,
    ctx: &QualitativeContext,
    level: &mut SignalLevel,
    rules: &mut Vec<String>,
    actions: &mut Vec<String>,
    directive: &mut FlowRedirectDirective,
) -> bool {
    let t = &config.thresholds;
    let prev = ctx.previous_levels.get(&AssetId::Nvda).copied();
    let dd = snap.drawdown_52w_pct();
    let var30 = snap.change_30d_pct;

    let dd_red = effective_threshold_with_hysteresis(
        t.nvda_dd_red_pct,
        prev,
        SignalLevel::Rosso,
        t.hysteresis_pct,
    );
    let dd_yellow = effective_threshold_with_hysteresis(
        t.nvda_dd_yellow_pct,
        prev,
        SignalLevel::Giallo,
        t.hysteresis_pct,
    );

    if dd > dd_red {
        *level = (*level).max(SignalLevel::Rosso);
        let red_pct = t.nvda_dd_red_pct;
        rules.push(format!("NVDA Drawdown 52w al {dd:.2}% (> {red_pct}%)"));
        if ctx.nvda_thesis_confirmed {
            actions.push(
                "Tesi confermata nella checklist: mantieni il PAC invariato su NVDA (mai acquisti extra oltre il PAC)."
                    .to_string(),
            );
        } else {
            directive.redirect_all_nvda_to_etf = true;
            actions.push(
                "Checklist di tesi NON confermata: dirotta l'intera quota PAC di NVDA sull'ETF S&P 500."
                    .to_string(),
            );
        }
        return true;
    }

    if var30 < t.nvda_var30_drop_yellow_pct || dd > dd_yellow {
        *level = (*level).max(SignalLevel::Giallo);
        let drop_y = t.nvda_var30_drop_yellow_pct;
        let dd_y = t.nvda_dd_yellow_pct;
        rules.push(format!(
            "NVDA variazione 30g {var30:+.2}% / drawdown {dd:.2}% (soglie {drop_y}% / {dd_y}%)"
        ));
        actions.push(
            "PAC invariato (abbassa il prezzo medio di carico) e rivedi la checklist di tesi NVDA. Mai acquisti extra oltre il PAC."
                .to_string(),
        );
        return true;
    }

    if var30 > t.nvda_var30_rally_yellow_pct {
        *level = (*level).max(SignalLevel::Giallo);
        let rally_y = t.nvda_var30_rally_yellow_pct;
        rules.push(format!("NVDA rialzo 30g al {var30:+.2}% (> +{rally_y}%)"));
        actions.push(
            "Non inseguire il rialzo: mantieni il solo PAC programmato e controlla il peso."
                .to_string(),
        );
    }
    false
}

fn evaluate_nvda(
    config: &PortfolioConfig,
    valuation: &PortfolioValuation,
    snap_opt: Option<&MarketSnapshot>,
    ctx: &QualitativeContext,
    directive: &mut FlowRedirectDirective,
) -> AssetSignalEvaluation {
    let t = &config.thresholds;
    let pos = &valuation.positions[&AssetId::Nvda];
    let month = ctx.current_pac_month.max(1);

    let mut level = SignalLevel::Verde;
    let mut rules = Vec::new();
    let mut actions = Vec::new();

    if month >= t.nvda_abs_weight_red_month && pos.actual_weight_pct > t.nvda_abs_weight_red_pct {
        level = level.max(SignalLevel::Rosso);
        let m_red = t.nvda_abs_weight_red_month;
        let w_act = pos.actual_weight_pct;
        let w_red = t.nvda_abs_weight_red_pct;
        rules.push(format!("Dal mese {m_red} peso assoluto NVDA al {w_act:.2}% (> {w_red}%)"));
        actions.push(
            "Valuta alleggerimento parziale nel calcolatore fiscale (imposta 26% sui gain) a confronto con il solo dirottamento dei flussi PAC."
                .to_string(),
        );
    }

    if pos.diff_vs_expected_pp > t.nvda_overweight_vs_expected_pp {
        level = level.max(SignalLevel::Giallo);
        directive.redirect_half_nvda_to_etf = true;
        let w_act = pos.actual_weight_pct;
        let diff = pos.diff_vs_expected_pp;
        let w_exp = pos.expected_weight_pct;
        let lim = t.nvda_overweight_vs_expected_pp;
        rules.push(format!(
            "Peso NVDA ({w_act:.2}%) superiore di {diff:+.2} pp rispetto al peso atteso ({w_exp:.2}%, soglia +{lim} pp)"
        ));
        actions.push(
            "Dirotta metà della quota PAC di NVDA verso l'ETF S&P 500 finché il peso non rientra."
                .to_string(),
        );
    }

    let checklist_required = snap_opt.is_some_and(|snap| {
        evaluate_nvda_market_rules(
            config,
            snap,
            ctx,
            &mut level,
            &mut rules,
            &mut actions,
            directive,
        )
    });

    if rules.is_empty() {
        rules.push(
            "NVDA entro le bande ordinarie di variazione, drawdown e peso atteso.".to_string(),
        );
    }
    if actions.is_empty() {
        actions.push("PAC ordinario invariato su NVDA.".to_string());
    }

    AssetSignalEvaluation {
        asset: AssetId::Nvda,
        level,
        triggered_rule: rules.join(" | "),
        suggested_pac_action: actions.join(" "),
        suggested_etf_advance_eur: None,
        covered_months_after_advance: None,
        checklist_review_required: checklist_required,
    }
}

fn evaluate_mp(
    config: &PortfolioConfig,
    valuation: &PortfolioValuation,
    snap_opt: Option<&MarketSnapshot>,
    ctx: &QualitativeContext,
    directive: &mut FlowRedirectDirective,
) -> AssetSignalEvaluation {
    let t = &config.thresholds;
    let prev = ctx.previous_levels.get(&AssetId::Mp).copied();
    let pos = &valuation.positions[&AssetId::Mp];
    let month = ctx.current_pac_month.max(1);

    let mut level = SignalLevel::Verde;
    let mut rules = Vec::new();
    let mut actions = Vec::new();
    let mut checklist_required = false;

    if ctx.mp_thesis_compromised {
        level = SignalLevel::Rosso;
        directive.redirect_all_mp_to_etf = true;
        rules.push("Tesi fondamentale MP Materials segnalata come compromessa".to_string());
        actions.push("Dirotta l'intera quota PAC di MP (5%) verso l'ETF S&P 500.".to_string());
    }

    if month >= t.mp_abs_weight_red_month && pos.actual_weight_pct > t.mp_abs_weight_red_pct {
        level = level.max(SignalLevel::Rosso);
        let m_red = t.mp_abs_weight_red_month;
        let w_act = pos.actual_weight_pct;
        let w_red = t.mp_abs_weight_red_pct;
        rules.push(format!("Dal mese {m_red} peso assoluto MP al {w_act:.2}% (> {w_red}%)"));
        actions.push(
            "Valuta alleggerimento parziale nel calcolatore fiscale (imposta 26% inclusa) rispetto al dirottamento dei flussi."
                .to_string(),
        );
    }

    if pos.diff_vs_expected_pp > t.mp_overweight_vs_expected_pp {
        level = level.max(SignalLevel::Giallo);
        directive.redirect_all_mp_to_etf = true;
        let w_act = pos.actual_weight_pct;
        let diff = pos.diff_vs_expected_pp;
        let w_exp = pos.expected_weight_pct;
        let lim = t.mp_overweight_vs_expected_pp;
        rules.push(format!(
            "Peso MP ({w_act:.2}%) superiore di {diff:+.2} pp rispetto al peso atteso ({w_exp:.2}%, soglia +{lim} pp)"
        ));
        actions.push(
            "Dirotta la quota PAC di MP verso l'ETF S&P 500 finché il peso non rientra."
                .to_string(),
        );
    }

    if let Some(snap) = snap_opt {
        let dd = snap.drawdown_52w_pct();
        let var30 = snap.change_30d_pct;
        let dd_yellow = effective_threshold_with_hysteresis(
            t.mp_dd_yellow_pct,
            prev,
            SignalLevel::Giallo,
            t.hysteresis_pct,
        );

        if var30 < t.mp_var30_drop_yellow_pct || dd > dd_yellow {
            level = level.max(SignalLevel::Giallo);
            checklist_required = true;
            let drop_y = t.mp_var30_drop_yellow_pct;
            let dd_y = t.mp_dd_yellow_pct;
            rules.push(format!(
                "MP variazione 30g {var30:+.2}% / drawdown {dd:.2}% (soglie {drop_y}% / {dd_y}%)"
            ));
            actions.push(
                "PAC invariato e verifica checklist MP (prezzo NdPr, accordo governo USA, rampa magneti, cassa/diluizione, concorrenza). Mai acquisti extra oltre il PAC."
                    .to_string(),
            );
        } else if var30 > t.mp_var30_rally_yellow_pct {
            level = level.max(SignalLevel::Giallo);
            let rally_y = t.mp_var30_rally_yellow_pct;
            rules.push(format!("MP rialzo 30g al {var30:+.2}% (> +{rally_y}%)"));
            actions.push(
                "Rialzo rapido: mantieni il solo PAC ordinario senza acquisti extra.".to_string(),
            );
        }
    }

    if rules.is_empty() {
        rules.push("MP Materials entro le bande ordinarie di variazione, drawdown e peso atteso.".to_string());
    }
    if actions.is_empty() {
        actions.push("PAC ordinario invariato su MP.".to_string());
    }

    AssetSignalEvaluation {
        asset: AssetId::Mp,
        level,
        triggered_rule: rules.join(" | "),
        suggested_pac_action: actions.join(" "),
        suggested_etf_advance_eur: None,
        covered_months_after_advance: None,
        checklist_review_required: checklist_required,
    }
}

pub fn is_phase_transition_month(config: &PortfolioConfig, month_number: u32) -> bool {
    month_number == config.pac.phase1_months + 1
}

pub fn phase_label(phase: PacPhase) -> &'static str {
    match phase {
        PacPhase::Phase1TrLiquidity => "Fase 1 (250 €/mese da liquidità Trade Republic)",
        PacPhase::Phase2CurrentSavings => "Fase 2 (100 €/mese da risparmi correnti)",
    }
}