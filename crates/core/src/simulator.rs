use crate::config::PortfolioConfig;
use crate::error::CoreError;
use crate::pac::calculate_base_pac_allocation;
use crate::tax::calculate_net_unrealized_pnl;
use crate::types::AssetId;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

pub const SCENARIO_DISCLAIMER: &str =
    "IPOTESI DI SIMULAZIONE A RENDIMENTO COSTANTE, NON PREVISIONI DI MERCATO.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScenarioKind {
    Prudente,
    Base,
    Ottimistico,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnualReturnAssumptions {
    pub kind: ScenarioKind,
    pub label: &'static str,
    pub etf_annual_pct: Decimal,
    pub nvda_annual_pct: Decimal,
    pub mp_annual_pct: Decimal,
}

impl AnnualReturnAssumptions {
    pub fn default_scenarios() -> [Self; 3] {
        [
            Self {
                kind: ScenarioKind::Prudente,
                label: "Prudente (ipotesi, non previsione)",
                etf_annual_pct: dec!(3.0),
                nvda_annual_pct: dec!(0.0),
                mp_annual_pct: dec!(-10.0),
            },
            Self {
                kind: ScenarioKind::Base,
                label: "Base (ipotesi, non previsione)",
                etf_annual_pct: dec!(6.5),
                nvda_annual_pct: dec!(8.0),
                mp_annual_pct: dec!(8.0),
            },
            Self {
                kind: ScenarioKind::Ottimistico,
                label: "Ottimistico (ipotesi, non previsione)",
                etf_annual_pct: dec!(9.0),
                nvda_annual_pct: dec!(15.0),
                mp_annual_pct: dec!(20.0),
            },
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulatedMonthPoint {
    pub month: u32,
    pub cumulative_invested_eur: Decimal,
    pub portfolio_value_eur: Decimal,
    pub net_value_after_26pct_tax_eur: Decimal,
    pub etf_weight_pct: Decimal,
    pub nvda_weight_pct: Decimal,
    pub mp_weight_pct: Decimal,
    pub remaining_tr_cash_eur: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationResult {
    pub disclaimer: &'static str,
    pub assumptions: AnnualReturnAssumptions,
    pub timeline: Vec<SimulatedMonthPoint>,
}

fn monthly_growth_factor(annual_pct: Decimal) -> Decimal {
    use rust_decimal::MathematicalOps;
    let annual_rate = dec!(1) + (annual_pct / dec!(100));
    if annual_rate <= dec!(0) {
        return dec!(0);
    }
    let ln_rate = annual_rate.ln();
    let monthly_ln = ln_rate / dec!(12);
    monthly_ln.exp()
}

pub fn run_pac_simulation(
    config: &PortfolioConfig,
    assumptions: AnnualReturnAssumptions,
    horizon_months: u32,
) -> Result<SimulationResult, CoreError> {
    if horizon_months == 0 {
        return Err(CoreError::InvalidMonthNumber(horizon_months));
    }

    let g_etf = monthly_growth_factor(assumptions.etf_annual_pct);
    let g_nvda = monthly_growth_factor(assumptions.nvda_annual_pct);
    let g_mp = monthly_growth_factor(assumptions.mp_annual_pct);

    let mut cost_etf = dec!(0);
    let mut cost_nvda = config.profile.initial_nvda_eur;
    let mut cost_mp = config.profile.initial_mp_eur;

    let mut val_etf = dec!(0);
    let mut val_nvda = config.profile.initial_nvda_eur;
    let mut val_mp = config.profile.initial_mp_eur;

    let mut tr_cash = config.profile.initial_tr_cash_eur;
    let mut timeline = Vec::with_capacity(horizon_months as usize);

    for m in 1..=horizon_months {
        val_etf *= g_etf;
        val_nvda *= g_nvda;
        val_mp *= g_mp;

        let alloc = calculate_base_pac_allocation(config, m)?;
        cost_etf += alloc.etf_sp500_eur;
        cost_nvda += alloc.nvda_eur;
        cost_mp += alloc.mp_eur;

        val_etf += alloc.etf_sp500_eur;
        val_nvda += alloc.nvda_eur;
        val_mp += alloc.mp_eur;

        if m <= config.pac.phase1_months {
            tr_cash = (tr_cash - alloc.total_eur).max(dec!(0));
        }

        let total_invested = (cost_etf + cost_nvda + cost_mp).round_dp(2);
        let total_val = (val_etf + val_nvda + val_mp).round_dp(2);

        let tax_etf = calculate_net_unrealized_pnl(cost_etf, val_etf, config)?.estimated_tax_eur;
        let tax_nvda =
            calculate_net_unrealized_pnl(cost_nvda, val_nvda, config)?.estimated_tax_eur;
        let tax_mp = calculate_net_unrealized_pnl(cost_mp, val_mp, config)?.estimated_tax_eur;
        let net_val = (total_val - tax_etf - tax_nvda - tax_mp).round_dp(2);

        let (w_etf, w_nvda, w_mp) = if total_val > dec!(0) {
            let wn = (val_nvda / total_val * dec!(100)).round_dp(2);
            let wm = (val_mp / total_val * dec!(100)).round_dp(2);
            let we = (dec!(100) - wn - wm).round_dp(2);
            (we, wn, wm)
        } else {
            (dec!(0), dec!(0), dec!(0))
        };

        let _ = AssetId::ALL;
        timeline.push(SimulatedMonthPoint {
            month: m,
            cumulative_invested_eur: total_invested,
            portfolio_value_eur: total_val,
            net_value_after_26pct_tax_eur: net_val,
            etf_weight_pct: w_etf,
            nvda_weight_pct: w_nvda,
            mp_weight_pct: w_mp,
            remaining_tr_cash_eur: tr_cash.round_dp(2),
        });
    }

    Ok(SimulationResult {
        disclaimer: SCENARIO_DISCLAIMER,
        assumptions,
        timeline,
    })
}