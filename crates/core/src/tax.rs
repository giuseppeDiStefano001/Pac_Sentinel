use crate::config::PortfolioConfig;
use crate::error::CoreError;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// Risultato della valutazione fiscale su una singola posizione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionTaxBreakdown {
    pub cost_basis_eur: Decimal,
    pub market_value_eur: Decimal,
    pub gross_pnl_eur: Decimal,
    /// Imposta sostitutiva 26%: applicata SOLO se `gross_pnl_eur > 0`.
    pub estimated_tax_eur: Decimal,
    pub net_pnl_eur: Decimal,
}

/// Simulazione completa di vendita ipotetica parziale o totale (regime amministrato).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypotheticalSaleSimulation {
    pub quantity_to_sell: Decimal,
    pub sale_price_eur: Decimal,
    pub gross_proceeds_eur: Decimal,
    pub proportional_cost_basis_eur: Decimal,
    pub capital_gain_eur: Decimal,
    pub capital_gains_tax_eur: Decimal,
    pub net_proceeds_eur: Decimal,
    pub remaining_quantity: Decimal,
    /// Confronto qualitativo/quantitativo con il solo ribilanciamento tramite flussi PAC.
    pub flow_rebalance_comparison: String,
}

/// Calcola P&L netto e imposta teorica su una posizione intera (solo su plusvalenze positive).
pub fn calculate_net_unrealized_pnl(
    cost_basis_eur: Decimal,
    market_value_eur: Decimal,
    config: &PortfolioConfig,
) -> Result<PositionTaxBreakdown, CoreError> {
    if cost_basis_eur < dec!(0) {
        return Err(CoreError::NegativeValue {
            field: "cost_basis_eur",
            value: cost_basis_eur,
        });
    }
    if market_value_eur < dec!(0) {
        return Err(CoreError::NegativeValue {
            field: "market_value_eur",
            value: market_value_eur,
        });
    }

    let gross_pnl_eur = market_value_eur - cost_basis_eur;
    let estimated_tax_eur = if gross_pnl_eur > dec!(0) {
        (gross_pnl_eur * config.profile.capital_gains_tax_pct / dec!(100)).round_dp(2)
    } else {
        dec!(0)
    };
    let net_pnl_eur = gross_pnl_eur - estimated_tax_eur;

    Ok(PositionTaxBreakdown {
        cost_basis_eur,
        market_value_eur,
        gross_pnl_eur,
        estimated_tax_eur,
        net_pnl_eur,
    })
}

/// Simula la vendita di `quantity_to_sell` quote al prezzo `sale_price_eur`,
/// calcolando l'imposta sostitutiva al 26% sul gain proporzionale al PMC.
pub fn simulate_hypothetical_sale(
    total_quantity: Decimal,
    average_cost_eur: Decimal,
    quantity_to_sell: Decimal,
    sale_price_eur: Decimal,
    monthly_pac_redirectable_eur: Decimal,
    config: &PortfolioConfig,
) -> Result<HypotheticalSaleSimulation, CoreError> {
    if quantity_to_sell <= dec!(0) {
        return Err(CoreError::NegativeValue {
            field: "quantity_to_sell",
            value: quantity_to_sell,
        });
    }
    if quantity_to_sell > total_quantity {
        return Err(CoreError::InsufficientQuantity {
            requested: quantity_to_sell,
            available: total_quantity,
        });
    }
    if sale_price_eur < dec!(0) {
        return Err(CoreError::NegativeValue {
            field: "sale_price_eur",
            value: sale_price_eur,
        });
    }

    let gross_proceeds_eur = (quantity_to_sell * sale_price_eur).round_dp(2);
    let proportional_cost_basis_eur = (quantity_to_sell * average_cost_eur).round_dp(2);
    let capital_gain_eur = gross_proceeds_eur - proportional_cost_basis_eur;

    let capital_gains_tax_eur = if capital_gain_eur > dec!(0) {
        (capital_gain_eur * config.profile.capital_gains_tax_pct / dec!(100)).round_dp(2)
    } else {
        dec!(0)
    };

    let net_proceeds_eur = gross_proceeds_eur - capital_gains_tax_eur;
    let remaining_quantity = total_quantity - quantity_to_sell;

    let comparison = if monthly_pac_redirectable_eur > dec!(0) {
        let months_equivalent = (net_proceeds_eur / monthly_pac_redirectable_eur).round_dp(1);
        format!(
            "La vendita genera un'imposta immediata e irrecuperabile di {capital_gains_tax_eur:.2} €. \
             Dirottando i flussi PAC ({monthly_pac_redirectable_eur:.2} €/mese) sull'ETF senza vendere, si riequilibra un controvalore equivalente \
             in circa {months_equivalent} mesi a 0 € di imposte e 0 € di commissioni."
        )
    } else {
        let tax_pct = config.profile.capital_gains_tax_pct;
        format!(
            "La vendita comporta un'imposta sostitutiva di {capital_gains_tax_eur:.2} € (aliquota {tax_pct}%)."
        )
    };

    Ok(HypotheticalSaleSimulation {
        quantity_to_sell,
        sale_price_eur,
        gross_proceeds_eur,
        proportional_cost_basis_eur,
        capital_gain_eur,
        capital_gains_tax_eur,
        net_proceeds_eur,
        remaining_quantity,
        flow_rebalance_comparison: comparison,
    })
}