use chrono::{NaiveDate, TimeZone};
use chrono_tz::Europe::Rome;
use pac_core::config::PortfolioConfig;
use pac_core::error::CoreError;
use pac_core::pac::{
    calculate_base_pac_allocation, covered_phase1_months, phase_for_month,
    remaining_tr_liquidity, PacPhase,
};
use pac_core::portfolio::{
    evaluate_emergency_fund, evaluate_portfolio, expected_weights_after_pac_months,
};
use pac_core::signals::{evaluate_signals, QualitativeContext};
use pac_core::simulator::{run_pac_simulation, AnnualReturnAssumptions};
use pac_core::tax::{calculate_net_unrealized_pnl, simulate_hypothetical_sale};
use pac_core::types::{AssetId, BuyOperation, Currency, MarketSnapshot, SignalLevel};
use rust_decimal_macros::dec;
use std::collections::BTreeMap;

#[test]
fn test_pac_phase1_allocation_250_eur() {
    let cfg = PortfolioConfig::default();
    let alloc = calculate_base_pac_allocation(&cfg, 1).expect("allocazione mese 1 valida");

    assert_eq!(alloc.phase, PacPhase::Phase1TrLiquidity);
    assert_eq!(alloc.total_eur, dec!(250));
    assert_eq!(alloc.etf_sp500_eur, dec!(200.00));
    assert_eq!(alloc.nvda_eur, dec!(37.50));
    assert_eq!(alloc.mp_eur, dec!(12.50));
}

#[test]
fn test_pac_phase2_allocation_100_eur_and_transition_at_month_25() {
    let cfg = PortfolioConfig::default();

    assert_eq!(
        phase_for_month(&cfg, 24).unwrap(),
        PacPhase::Phase1TrLiquidity
    );

    let alloc25 = calculate_base_pac_allocation(&cfg, 25).expect("allocazione mese 25 valida");
    assert_eq!(alloc25.phase, PacPhase::Phase2CurrentSavings);
    assert_eq!(alloc25.total_eur, dec!(100));
    assert_eq!(alloc25.etf_sp500_eur, dec!(80.00));
    assert_eq!(alloc25.nvda_eur, dec!(15.00));
    assert_eq!(alloc25.mp_eur, dec!(5.00));
}

#[test]
fn test_6000_eur_tr_liquidity_exhausted_after_24_installments() {
    let cfg = PortfolioConfig::default();

    let after_12 = remaining_tr_liquidity(&cfg, 12, dec!(0)).unwrap();
    assert_eq!(after_12, dec!(3000));
    assert_eq!(covered_phase1_months(&cfg, after_12), 12);

    let after_24 = remaining_tr_liquidity(&cfg, 24, dec!(0)).unwrap();
    assert_eq!(after_24, dec!(0));
    assert_eq!(covered_phase1_months(&cfg, after_24), 0);
}

#[test]
fn test_emergency_fund_is_strictly_isolated_from_all_calculations() {
    let mut cfg1 = PortfolioConfig::default();
    cfg1.profile.emergency_fund_eur = dec!(4592);

    let mut cfg2 = PortfolioConfig::default();
    cfg2.profile.emergency_fund_eur = dec!(25000);

    assert_eq!(
        calculate_base_pac_allocation(&cfg1, 1).unwrap(),
        calculate_base_pac_allocation(&cfg2, 1).unwrap()
    );

    let cash1 = remaining_tr_liquidity(&cfg1, 5, dec!(0)).unwrap();
    let cash2 = remaining_tr_liquidity(&cfg2, 5, dec!(0)).unwrap();
    assert_eq!(cash1, cash2);
    assert_eq!(
        covered_phase1_months(&cfg1, cash1),
        covered_phase1_months(&cfg2, cash2)
    );

    let ops = vec![
        BuyOperation {
            id: Some(1),
            asset: AssetId::Nvda,
            date: NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            invested_eur: dec!(300),
            unit_price_native: dec!(120),
            native_currency: Currency::Usd,
            eur_usd_rate: dec!(1.08),
            quantity: dec!(2.70),
            note: None,
        },
        BuyOperation {
            id: Some(2),
            asset: AssetId::Mp,
            date: NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            invested_eur: dec!(100),
            unit_price_native: dec!(20),
            native_currency: Currency::Usd,
            eur_usd_rate: dec!(1.08),
            quantity: dec!(5.40),
            note: None,
        },
    ];
    let snapshots = sample_snapshots(dec!(500), dec!(120), dec!(20));
    let val1 = evaluate_portfolio(&cfg1, &ops, &snapshots, 0).unwrap();
    let val2 = evaluate_portfolio(&cfg2, &ops, &snapshots, 0).unwrap();
    assert_eq!(val1, val2);

    let ef1 = evaluate_emergency_fund(&cfg1);
    assert!(!ef1.is_above_warning_threshold);

    let ef2 = evaluate_emergency_fund(&cfg2);
    assert!(ef2.is_above_warning_threshold);
}

#[test]
fn test_tax_26_percent_only_on_positive_capital_gains() {
    let cfg = PortfolioConfig::default();

    let gain = calculate_net_unrealized_pnl(dec!(2000), dec!(3000), &cfg).unwrap();
    assert_eq!(gain.gross_pnl_eur, dec!(1000));
    assert_eq!(gain.estimated_tax_eur, dec!(260.00));
    assert_eq!(gain.net_pnl_eur, dec!(740.00));

    let loss = calculate_net_unrealized_pnl(dec!(2000), dec!(1500), &cfg).unwrap();
    assert_eq!(loss.gross_pnl_eur, dec!(-500));
    assert_eq!(loss.estimated_tax_eur, dec!(0));
    assert_eq!(loss.net_pnl_eur, dec!(-500));

    let even = calculate_net_unrealized_pnl(dec!(2000), dec!(2000), &cfg).unwrap();
    assert_eq!(even.estimated_tax_eur, dec!(0));

    let sale = simulate_hypothetical_sale(
        dec!(10),
        dec!(100),
        dec!(4),
        dec!(150),
        dec!(37.50),
        &cfg,
    )
    .unwrap();
    assert_eq!(sale.gross_proceeds_eur, dec!(600.00));
    assert_eq!(sale.capital_gain_eur, dec!(200.00));
    assert_eq!(sale.capital_gains_tax_eur, dec!(52.00));
    assert_eq!(sale.net_proceeds_eur, dec!(548.00));
    assert_eq!(sale.remaining_quantity, dec!(6));
}

#[test]
fn test_expected_weights_convergence_from_initial_satellite_overweight() {
    let cfg = PortfolioConfig::default();

    let w0 = expected_weights_after_pac_months(&cfg, 0).unwrap();
    assert_eq!(w0[&AssetId::EtfSp500], dec!(0.00));
    assert_eq!(w0[&AssetId::Nvda], dec!(75.00));
    assert_eq!(w0[&AssetId::Mp], dec!(25.00));

    let w24 = expected_weights_after_pac_months(&cfg, 24).unwrap();
    assert_eq!(w24[&AssetId::EtfSp500], dec!(75.00));
    assert_eq!(w24[&AssetId::Nvda], dec!(18.75));
    assert_eq!(w24[&AssetId::Mp], dec!(6.25));
}

fn fixture_drawdown_and_overweight() -> (Vec<BuyOperation>, BTreeMap<AssetId, MarketSnapshot>) {
    let ops = vec![
        BuyOperation {
            id: Some(1),
            asset: AssetId::EtfSp500,
            date: NaiveDate::from_ymd_opt(2026, 2, 2).unwrap(),
            invested_eur: dec!(200),
            unit_price_native: dec!(400),
            native_currency: Currency::Eur,
            eur_usd_rate: dec!(1),
            quantity: dec!(0.50),
            note: None,
        },
        BuyOperation {
            id: Some(2),
            asset: AssetId::Nvda,
            date: NaiveDate::from_ymd_opt(2026, 2, 2).unwrap(),
            invested_eur: dec!(337.50),
            unit_price_native: dec!(100),
            native_currency: Currency::Usd,
            eur_usd_rate: dec!(1),
            quantity: dec!(3.375),
            note: None,
        },
        BuyOperation {
            id: Some(3),
            asset: AssetId::Mp,
            date: NaiveDate::from_ymd_opt(2026, 2, 2).unwrap(),
            invested_eur: dec!(112.50),
            unit_price_native: dec!(20),
            native_currency: Currency::Usd,
            eur_usd_rate: dec!(1),
            quantity: dec!(5.625),
            note: None,
        },
    ];

    let ts = Rome.with_ymd_and_hms(2026, 3, 2, 16, 0, 0).unwrap();
    let mut snapshots = BTreeMap::new();
    snapshots.insert(
        AssetId::EtfSp500,
        MarketSnapshot {
            asset: AssetId::EtfSp500,
            price_native: dec!(400),
            currency: Currency::Eur,
            price_eur: dec!(400),
            change_7d_pct: dec!(-4.0),
            change_30d_pct: dec!(-12.0),
            high_52w_native: dec!(533.33),
            source: "test".to_string(),
            timestamp_rome: ts,
            is_stale: false,
        },
    );
    snapshots.insert(
        AssetId::Nvda,
        MarketSnapshot {
            asset: AssetId::Nvda,
            price_native: dec!(160),
            currency: Currency::Usd,
            price_eur: dec!(160),
            change_7d_pct: dec!(5.0),
            change_30d_pct: dec!(20.0),
            high_52w_native: dec!(165),
            source: "test".to_string(),
            timestamp_rome: ts,
            is_stale: false,
        },
    );
    snapshots.insert(
        AssetId::Mp,
        MarketSnapshot {
            asset: AssetId::Mp,
            price_native: dec!(20),
            currency: Currency::Usd,
            price_eur: dec!(20),
            change_7d_pct: dec!(0.0),
            change_30d_pct: dec!(2.0),
            high_52w_native: dec!(25),
            source: "test".to_string(),
            timestamp_rome: ts,
            is_stale: false,
        },
    );
    (ops, snapshots)
}

#[test]
fn test_signals_etf_drawdown_tiers_and_flow_redirection() {
    let cfg = PortfolioConfig::default();
    let (ops, snapshots) = fixture_drawdown_and_overweight();
    let valuation = evaluate_portfolio(&cfg, &ops, &snapshots, 1).unwrap();
    let ctx = QualitativeContext {
        current_pac_month: 2,
        remaining_tr_cash_eur: dec!(5750),
        cumulative_etf_advance_used_pct: dec!(0),
        last_etf_advance_date: None,
        evaluation_date: Some(NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()),
        nvda_thesis_confirmed: true,
        mp_thesis_compromised: false,
        previous_levels: BTreeMap::new(),
    };

    let report = evaluate_signals(&cfg, &valuation, &snapshots, &ctx).unwrap();

    let etf_sig = &report.asset_signals[&AssetId::EtfSp500];
    assert_eq!(etf_sig.level, SignalLevel::Arancione);
    assert_eq!(etf_sig.suggested_etf_advance_eur, Some(dec!(1150.00)));
    assert_eq!(etf_sig.covered_months_after_advance, Some(18));

    assert_eq!(report.adjusted_pac_allocation.nvda_eur, dec!(18.75));
    assert_eq!(report.adjusted_pac_allocation.etf_sp500_eur, dec!(218.75));
}

#[test]
fn test_config_validation_rejects_wrong_weights_and_invalid_day() {
    let mut cfg = PortfolioConfig::default();
    cfg.weights.etf_sp500_pct = dec!(70);
    assert!(matches!(
        cfg.validate(),
        Err(CoreError::InvalidTargetWeights(_))
    ));

    let mut cfg_day = PortfolioConfig::default();
    cfg_day.pac.execution_day_of_month = 31;
    assert!(matches!(
        cfg_day.validate(),
        Err(CoreError::InvalidPacDay(31))
    ));
}

#[test]
fn test_simulator_runs_120_months_cleanly() {
    let cfg = PortfolioConfig::default();
    let scenarios = AnnualReturnAssumptions::default_scenarios();
    let res = run_pac_simulation(&cfg, scenarios[1], 120).unwrap();

    assert_eq!(res.timeline.len(), 120);
    assert_eq!(res.timeline[23].remaining_tr_cash_eur, dec!(0.00));
    assert_eq!(res.timeline[119].cumulative_invested_eur, dec!(16000.00));
}

fn sample_snapshots(
    etf_eur: rust_decimal::Decimal,
    nvda_usd: rust_decimal::Decimal,
    mp_usd: rust_decimal::Decimal,
) -> BTreeMap<AssetId, MarketSnapshot> {
    let ts = Rome.with_ymd_and_hms(2026, 2, 10, 15, 0, 0).unwrap();
    let mut map = BTreeMap::new();
    map.insert(
        AssetId::EtfSp500,
        MarketSnapshot {
            asset: AssetId::EtfSp500,
            price_native: etf_eur,
            currency: Currency::Eur,
            price_eur: etf_eur,
            change_7d_pct: dec!(0),
            change_30d_pct: dec!(0),
            high_52w_native: etf_eur,
            source: "test".to_string(),
            timestamp_rome: ts,
            is_stale: false,
        },
    );
    map.insert(
        AssetId::Nvda,
        MarketSnapshot {
            asset: AssetId::Nvda,
            price_native: nvda_usd,
            currency: Currency::Usd,
            price_eur: nvda_usd,
            change_7d_pct: dec!(0),
            change_30d_pct: dec!(0),
            high_52w_native: nvda_usd,
            source: "test".to_string(),
            timestamp_rome: ts,
            is_stale: false,
        },
    );
    map.insert(
        AssetId::Mp,
        MarketSnapshot {
            asset: AssetId::Mp,
            price_native: mp_usd,
            currency: Currency::Usd,
            price_eur: mp_usd,
            change_7d_pct: dec!(0),
            change_30d_pct: dec!(0),
            high_52w_native: mp_usd,
            source: "test".to_string(),
            timestamp_rome: ts,
            is_stale: false,
        },
    );
    map
}