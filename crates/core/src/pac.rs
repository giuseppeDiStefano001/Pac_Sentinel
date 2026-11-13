use crate::config::PortfolioConfig;
use crate::error::CoreError;
use crate::types::AssetId;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PacPhase {
    /// Mesi 1..=24: finanziato dal saldo liquido iniziale su Trade Republic.
    Phase1TrLiquidity,
    /// Dal mese 25 in poi: finanziato dai risparmi correnti mensili.
    Phase2CurrentSavings,
}

/// Ripartizione dettagliata in EUR di una singola rata PAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacAllocation {
    pub month_number: u32,
    pub phase: PacPhase,
    pub total_eur: Decimal,
    pub etf_sp500_eur: Decimal,
    pub nvda_eur: Decimal,
    pub mp_eur: Decimal,
}

impl PacAllocation {
    pub fn amount_for(&self, asset: AssetId) -> Decimal {
        match asset {
            AssetId::EtfSp500 => self.etf_sp500_eur,
            AssetId::Nvda => self.nvda_eur,
            AssetId::Mp => self.mp_eur,
        }
    }
}

/// Istruzioni operative di dirottamento dei flussi PAC quando scattano regole di riequilibrio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FlowRedirectDirective {
    /// Dirotta metà della quota NVDA sull'ETF (peso NVDA > +5 pp rispetto al peso atteso).
    pub redirect_half_nvda_to_etf: bool,
    /// Dirotta l'intera quota NVDA sull'ETF (drawdown > 50% con tesi non confermata).
    pub redirect_all_nvda_to_etf: bool,
    /// Dirotta l'intera quota MP sull'ETF (peso MP > +3 pp rispetto al peso atteso o tesi compromessa).
    pub redirect_all_mp_to_etf: bool,
}

/// Determina la fase attiva dato il numero del mese del piano (1-based).
pub fn phase_for_month(config: &PortfolioConfig, month_number: u32) -> Result<PacPhase, CoreError> {
    if month_number == 0 {
        return Err(CoreError::InvalidMonthNumber(month_number));
    }
    if month_number <= config.pac.phase1_months {
        Ok(PacPhase::Phase1TrLiquidity)
    } else {
        Ok(PacPhase::Phase2CurrentSavings)
    }
}

/// Calcola la rata PAC nominale per il mese specificato (1-based).
/// NOTA: il fondo emergenze non viene MAI letto né utilizzato.
pub fn calculate_base_pac_allocation(
    config: &PortfolioConfig,
    month_number: u32,
) -> Result<PacAllocation, CoreError> {
    let phase = phase_for_month(config, month_number)?;
    let total_eur = match phase {
        PacPhase::Phase1TrLiquidity => config.pac.phase1_monthly_eur,
        PacPhase::Phase2CurrentSavings => config.pac.phase2_monthly_eur,
    };

    let nvda_eur = (total_eur * config.weights.nvda_pct / dec!(100)).round_dp(2);
    let mp_eur = (total_eur * config.weights.mp_pct / dec!(100)).round_dp(2);
    // L'ETF assorbe il complemento esatto per evitare centesimi residui da arrotondamento.
    let etf_sp500_eur = total_eur - nvda_eur - mp_eur;

    Ok(PacAllocation {
        month_number,
        phase,
        total_eur,
        etf_sp500_eur,
        nvda_eur,
        mp_eur,
    })
}

/// Applica le direttive di dirottamento dei flussi PAC (senza vendere nulla).
pub fn apply_flow_redirection(
    base: PacAllocation,
    directive: FlowRedirectDirective,
) -> PacAllocation {
    let mut etf = base.etf_sp500_eur;
    let mut nvda = base.nvda_eur;
    let mut mp = base.mp_eur;

    if directive.redirect_all_nvda_to_etf {
        etf += nvda;
        nvda = dec!(0);
    } else if directive.redirect_half_nvda_to_etf {
        let shifted = (nvda / dec!(2)).round_dp(2);
        nvda -= shifted;
        etf += shifted;
    }

    if directive.redirect_all_mp_to_etf {
        etf += mp;
        mp = dec!(0);
    }

    PacAllocation {
        month_number: base.month_number,
        phase: base.phase,
        total_eur: base.total_eur,
        etf_sp500_eur: etf,
        nvda_eur: nvda,
        mp_eur: mp,
    }
}

/// Calcola la liquidità residua su Trade Republic dopo N rate di Fase 1 ed eventuali anticipi straordinari.
/// Ignora completamente il fondo emergenze.
pub fn remaining_tr_liquidity(
    config: &PortfolioConfig,
    phase1_installments_executed: u32,
    extra_advances_executed_eur: Decimal,
) -> Result<Decimal, CoreError> {
    if extra_advances_executed_eur < dec!(0) {
        return Err(CoreError::NegativeValue {
            field: "extra_advances_executed_eur",
            value: extra_advances_executed_eur,
        });
    }
    let spent_pac = config.pac.phase1_monthly_eur * Decimal::from(phase1_installments_executed);
    let remaining = config.profile.initial_tr_cash_eur - spent_pac - extra_advances_executed_eur;
    Ok(remaining.max(dec!(0)))
}

/// Calcola quante rate intere di Fase 1 sono ancora coperte dalla liquidità residua su Trade Republic.
pub fn covered_phase1_months(config: &PortfolioConfig, remaining_tr_cash_eur: Decimal) -> u32 {
    if remaining_tr_cash_eur <= dec!(0) || config.pac.phase1_monthly_eur <= dec!(0) {
        return 0;
    }
    let months_dec = (remaining_tr_cash_eur / config.pac.phase1_monthly_eur).floor();
    u32::try_from(months_dec).unwrap_or(0)
}