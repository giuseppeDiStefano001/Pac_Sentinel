use crate::config::PortfolioConfig;
use crate::error::CoreError;
use crate::pac::calculate_base_pac_allocation;
use crate::tax::calculate_net_unrealized_pnl;
use crate::types::{AssetId, BuyOperation, EmergencyFundStatus, MarketSnapshot};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Dettaglio analitico della posizione su un singolo strumento.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetPositionSummary {
    pub asset: AssetId,
    pub total_quantity: Decimal,
    pub total_invested_eur: Decimal,
    /// Prezzo medio di carico in EUR.
    pub average_cost_eur: Decimal,
    /// Prezzo medio di carico nella valuta nativa dello strumento.
    pub average_cost_native: Decimal,
    /// Distanza percentuale del prezzo corrente dal prezzo medio di carico nativo.
    pub distance_from_pmc_pct: Decimal,
    pub current_market_value_eur: Decimal,
    pub unrealized_gross_pnl_eur: Decimal,
    /// P&L netto dopo imposta sostitutiva 26% (applicata solo se plusvalenza > 0).
    pub unrealized_net_pnl_eur: Decimal,
    /// Peso reale sul totale investito a mercato (%).
    pub actual_weight_pct: Decimal,
    /// Peso atteso a costo storico dopo gli acquisti iniziali + N rate PAC (%).
    pub expected_weight_pct: Decimal,
    /// Peso target di lungo periodo (%).
    pub target_weight_pct: Decimal,
    /// Scostamento in punti percentuali: peso reale - peso atteso.
    pub diff_vs_expected_pp: Decimal,
}

/// Stato completo del portafoglio investito (escluso il fondo emergenze).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortfolioValuation {
    pub positions: BTreeMap<AssetId, AssetPositionSummary>,
    pub total_invested_eur: Decimal,
    pub total_market_value_eur: Decimal,
    pub total_gross_pnl_eur: Decimal,
    /// P&L netto calcolato posizione per posizione.
    pub total_net_pnl_eur: Decimal,
    /// Esposizione effettiva a NVDA = peso diretto NVDA + (peso ETF * peso NVDA nell'ETF).
    pub effective_nvda_exposure_pct: Decimal,
    /// Peso aggregato dei satelliti (NVDA + MP).
    pub satellites_weight_pct: Decimal,
}

struct AggregatedHoldings {
    qty: BTreeMap<AssetId, Decimal>,
    invested_eur: BTreeMap<AssetId, Decimal>,
    invested_native: BTreeMap<AssetId, Decimal>,
}

/// Restituisce lo stato informativo isolato del fondo emergenze.
pub fn evaluate_emergency_fund(config: &PortfolioConfig) -> EmergencyFundStatus {
    let balance = config.profile.emergency_fund_eur;
    let warn = config.profile.emergency_fund_warning_eur;
    EmergencyFundStatus {
        balance_eur: balance,
        stamp_duty_warning_threshold_eur: warn,
        stamp_duty_legal_threshold_eur: config.profile.emergency_fund_stamp_threshold_eur,
        is_above_warning_threshold: balance > warn,
    }
}

/// Calcola i pesi attesi teorici a costo storico (capitale iniziale 300€ NVDA + 100€ MP + rate PAC eseguite).
pub fn expected_weights_after_pac_months(
    config: &PortfolioConfig,
    completed_pac_months: u32,
) -> Result<BTreeMap<AssetId, Decimal>, CoreError> {
    let mut cost_etf = dec!(0);
    let mut cost_nvda = config.profile.initial_nvda_eur;
    let mut cost_mp = config.profile.initial_mp_eur;

    for m in 1..=completed_pac_months {
        let alloc = calculate_base_pac_allocation(config, m)?;
        cost_etf += alloc.etf_sp500_eur;
        cost_nvda += alloc.nvda_eur;
        cost_mp += alloc.mp_eur;
    }

    let total_cost = cost_etf + cost_nvda + cost_mp;
    let mut weights = BTreeMap::new();

    if total_cost <= dec!(0) {
        weights.insert(AssetId::EtfSp500, config.weights.etf_sp500_pct);
        weights.insert(AssetId::Nvda, config.weights.nvda_pct);
        weights.insert(AssetId::Mp, config.weights.mp_pct);
        return Ok(weights);
    }

    let w_nvda = (cost_nvda / total_cost * dec!(100)).round_dp(2);
    let w_mp = (cost_mp / total_cost * dec!(100)).round_dp(2);
    let w_etf = (dec!(100) - w_nvda - w_mp).round_dp(2);

    weights.insert(AssetId::EtfSp500, w_etf);
    weights.insert(AssetId::Nvda, w_nvda);
    weights.insert(AssetId::Mp, w_mp);

    Ok(weights)
}

fn aggregate_operations(operations: &[BuyOperation]) -> Result<AggregatedHoldings, CoreError> {
    let mut qty = BTreeMap::new();
    let mut invested_eur = BTreeMap::new();
    let mut invested_native = BTreeMap::new();

    for asset in AssetId::ALL {
        qty.insert(asset, dec!(0));
        invested_eur.insert(asset, dec!(0));
        invested_native.insert(asset, dec!(0));
    }

    for op in operations {
        if op.invested_eur < dec!(0) || op.quantity < dec!(0) || op.unit_price_native < dec!(0) {
            return Err(CoreError::NegativeValue {
                field: "operation_value",
                value: op.invested_eur,
            });
        }
        *qty.entry(op.asset).or_insert(dec!(0)) += op.quantity;
        *invested_eur.entry(op.asset).or_insert(dec!(0)) += op.invested_eur;
        *invested_native.entry(op.asset).or_insert(dec!(0)) += op.unit_price_native * op.quantity;
    }

    Ok(AggregatedHoldings {
        qty,
        invested_eur,
        invested_native,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_asset_summary(
    asset: AssetId,
    holdings: &AggregatedHoldings,
    mkt_val: Decimal,
    total_market_value_eur: Decimal,
    expected_weight_pct: Decimal,
    snapshot: Option<&MarketSnapshot>,
    config: &PortfolioConfig,
) -> Result<AssetPositionSummary, CoreError> {
    let qty = holdings.qty[&asset];
    let inv_eur = holdings.invested_eur[&asset];
    let inv_native = holdings.invested_native[&asset];

    let average_cost_eur = if qty > dec!(0) {
        (inv_eur / qty).round_dp(4)
    } else {
        dec!(0)
    };
    let average_cost_native = if qty > dec!(0) {
        (inv_native / qty).round_dp(4)
    } else {
        dec!(0)
    };

    let distance_from_pmc_pct = match snapshot {
        Some(snap) if average_cost_native > dec!(0) => {
            ((snap.price_native - average_cost_native) / average_cost_native * dec!(100)).round_dp(2)
        }
        _ => dec!(0),
    };

    let gross_pnl = mkt_val - inv_eur;
    let tax_breakdown = calculate_net_unrealized_pnl(inv_eur, mkt_val, config)?;

    let actual_weight_pct = if total_market_value_eur > dec!(0) {
        (mkt_val / total_market_value_eur * dec!(100)).round_dp(2)
    } else {
        dec!(0)
    };

    let target_weight_pct = match asset {
        AssetId::EtfSp500 => config.weights.etf_sp500_pct,
        AssetId::Nvda => config.weights.nvda_pct,
        AssetId::Mp => config.weights.mp_pct,
    };

    Ok(AssetPositionSummary {
        asset,
        total_quantity: qty,
        total_invested_eur: inv_eur,
        average_cost_eur,
        average_cost_native,
        distance_from_pmc_pct,
        current_market_value_eur: mkt_val,
        unrealized_gross_pnl_eur: gross_pnl,
        unrealized_net_pnl_eur: tax_breakdown.net_pnl_eur,
        actual_weight_pct,
        expected_weight_pct,
        target_weight_pct,
        diff_vs_expected_pp: (actual_weight_pct - expected_weight_pct).round_dp(2),
    })
}

/// Valuta il portafoglio aggregando le operazioni registrate e le quotazioni correnti.
/// Il fondo emergenze non entra in alcun calcolo.
pub fn evaluate_portfolio(
    config: &PortfolioConfig,
    operations: &[BuyOperation],
    snapshots: &BTreeMap<AssetId, MarketSnapshot>,
    completed_pac_months: u32,
) -> Result<PortfolioValuation, CoreError> {
    let expected_weights = expected_weights_after_pac_months(config, completed_pac_months)?;
    let holdings = aggregate_operations(operations)?;

    let mut market_values = BTreeMap::new();
    let mut total_market_value_eur = dec!(0);
    let mut total_invested_eur = dec!(0);

    for asset in AssetId::ALL {
        let price_eur = snapshots.get(&asset).map_or(dec!(0), |snap| snap.price_eur);
        let mkt_val = (holdings.qty[&asset] * price_eur).round_dp(2);
        market_values.insert(asset, mkt_val);
        total_market_value_eur += mkt_val;
        total_invested_eur += holdings.invested_eur[&asset];
    }

    let mut positions = BTreeMap::new();
    let mut total_gross_pnl_eur = dec!(0);
    let mut total_net_pnl_eur = dec!(0);

    for asset in AssetId::ALL {
        let summary = build_asset_summary(
            asset,
            &holdings,
            market_values[&asset],
            total_market_value_eur,
            expected_weights[&asset],
            snapshots.get(&asset),
            config,
        )?;
        total_gross_pnl_eur += summary.unrealized_gross_pnl_eur;
        total_net_pnl_eur += summary.unrealized_net_pnl_eur;
        positions.insert(asset, summary);
    }

    let etf_weight = positions[&AssetId::EtfSp500].actual_weight_pct;
    let nvda_direct_weight = positions[&AssetId::Nvda].actual_weight_pct;
    let mp_weight = positions[&AssetId::Mp].actual_weight_pct;

    let indirect_nvda = (etf_weight * config.weights.nvda_inside_etf_pct / dec!(100)).round_dp(2);
    let effective_nvda_exposure_pct = (nvda_direct_weight + indirect_nvda).round_dp(2);
    let satellites_weight_pct = (nvda_direct_weight + mp_weight).round_dp(2);

    Ok(PortfolioValuation {
        positions,
        total_invested_eur,
        total_market_value_eur,
        total_gross_pnl_eur,
        total_net_pnl_eur,
        effective_nvda_exposure_pct,
        satellites_weight_pct,
    })
}