use crate::error::CoreError;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortfolioConfig {
    pub profile: ProfileConfig,
    pub pac: PacConfig,
    pub weights: TargetWeightsConfig,
    pub thresholds: ThresholdsConfig,
    pub market: MarketScheduleConfig,
    pub notifications: NotificationPolicyConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub broker_name: String,
    pub regime_amministrato: bool,
    pub capital_gains_tax_pct: Decimal,
    pub emergency_fund_eur: Decimal,
    pub emergency_fund_warning_eur: Decimal,
    pub emergency_fund_stamp_threshold_eur: Decimal,
    pub initial_nvda_eur: Decimal,
    pub initial_mp_eur: Decimal,
    pub initial_tr_cash_eur: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacConfig {
    pub phase1_months: u32,
    pub phase1_monthly_eur: Decimal,
    pub phase2_monthly_eur: Decimal,
    pub execution_day_of_month: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetWeightsConfig {
    pub etf_sp500_pct: Decimal,
    pub nvda_pct: Decimal,
    pub mp_pct: Decimal,
    pub nvda_inside_etf_pct: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdsConfig {
    pub hysteresis_pct: Decimal,
    pub etf_var30_normal_band_pct: Decimal,
    pub etf_dd_yellow_min_pct: Decimal,
    pub etf_dd_orange_min_pct: Decimal,
    pub etf_dd_red_min_pct: Decimal,
    pub etf_advance_yellow_max_pct: Decimal,
    pub etf_advance_orange_cum_max_pct: Decimal,
    pub etf_advance_red_cum_max_pct: Decimal,
    pub etf_advance_min_days_between_tranches: u32,
    pub nvda_var30_drop_yellow_pct: Decimal,
    pub nvda_var30_rally_yellow_pct: Decimal,
    pub nvda_dd_yellow_pct: Decimal,
    pub nvda_dd_red_pct: Decimal,
    pub nvda_overweight_vs_expected_pp: Decimal,
    pub nvda_abs_weight_red_month: u32,
    pub nvda_abs_weight_red_pct: Decimal,
    pub mp_var30_drop_yellow_pct: Decimal,
    pub mp_var30_rally_yellow_pct: Decimal,
    pub mp_dd_yellow_pct: Decimal,
    pub mp_overweight_vs_expected_pp: Decimal,
    pub mp_abs_weight_red_month: u32,
    pub mp_abs_weight_red_pct: Decimal,
    pub portfolio_nvda_effective_max_pct: Decimal,
    pub portfolio_satellites_max_pct: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketScheduleConfig {
    pub etf_symbol: String,
    pub api_provider: String,
    pub poll_interval_minutes_market_open: u64,
    pub stale_after_hours: i64,
    pub history_days_initial_fetch: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicyConfig {
    /// ID numerico della chat Telegram autorizzata (allow-list esclusiva).
    #[serde(default)]
    pub authorized_chat_id: Option<i64>,
    pub cooldown_hours_same_level: i64,
    pub quiet_hours_start_hour: u32,
    pub quiet_hours_end_hour: u32,
    pub allow_red_during_quiet_hours: bool,
    pub low_cash_warning_remaining_months: u32,
    pub daily_summary_enabled: bool,
    pub daily_summary_hour_rome: u32,
}

impl Default for PortfolioConfig {
    fn default() -> Self {
        Self {
            profile: ProfileConfig {
                broker_name: "Trade Republic".to_string(),
                regime_amministrato: true,
                capital_gains_tax_pct: dec!(26),
                emergency_fund_eur: dec!(4592),
                emergency_fund_warning_eur: dec!(4800),
                emergency_fund_stamp_threshold_eur: dec!(5000),
                initial_nvda_eur: dec!(300),
                initial_mp_eur: dec!(100),
                initial_tr_cash_eur: dec!(6000),
            },
            pac: PacConfig {
                phase1_months: 24,
                phase1_monthly_eur: dec!(250),
                phase2_monthly_eur: dec!(100),
                execution_day_of_month: 2,
            },
            weights: TargetWeightsConfig {
                etf_sp500_pct: dec!(80),
                nvda_pct: dec!(15),
                mp_pct: dec!(5),
                nvda_inside_etf_pct: dec!(7),
            },
            thresholds: ThresholdsConfig {
                hysteresis_pct: dec!(1.0),
                etf_var30_normal_band_pct: dec!(10),
                etf_dd_yellow_min_pct: dec!(10),
                etf_dd_orange_min_pct: dec!(20),
                etf_dd_red_min_pct: dec!(30),
                etf_advance_yellow_max_pct: dec!(10),
                etf_advance_orange_cum_max_pct: dec!(20),
                etf_advance_red_cum_max_pct: dec!(35),
                etf_advance_min_days_between_tranches: 14,
                nvda_var30_drop_yellow_pct: dec!(-15),
                nvda_var30_rally_yellow_pct: dec!(30),
                nvda_dd_yellow_pct: dec!(30),
                nvda_dd_red_pct: dec!(50),
                nvda_overweight_vs_expected_pp: dec!(5),
                nvda_abs_weight_red_month: 18,
                nvda_abs_weight_red_pct: dec!(30),
                mp_var30_drop_yellow_pct: dec!(-25),
                mp_var30_rally_yellow_pct: dec!(40),
                mp_dd_yellow_pct: dec!(50),
                mp_overweight_vs_expected_pp: dec!(3),
                mp_abs_weight_red_month: 18,
                mp_abs_weight_red_pct: dec!(10),
                portfolio_nvda_effective_max_pct: dec!(25),
                portfolio_satellites_max_pct: dec!(30),
            },
            market: MarketScheduleConfig {
                etf_symbol: "CSSPX.MI".to_string(),
                api_provider: "twelvedata".to_string(),
                poll_interval_minutes_market_open: 15,
                stale_after_hours: 4,
                history_days_initial_fetch: 400,
            },
            notifications: NotificationPolicyConfig {
                authorized_chat_id: None,
                cooldown_hours_same_level: 12,
                quiet_hours_start_hour: 23,
                quiet_hours_end_hour: 7,
                allow_red_during_quiet_hours: true,
                low_cash_warning_remaining_months: 3,
                daily_summary_enabled: false,
                daily_summary_hour_rome: 19,
            },
        }
    }
}

impl PortfolioConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        let sum_weights =
            self.weights.etf_sp500_pct + self.weights.nvda_pct + self.weights.mp_pct;
        if sum_weights != dec!(100) {
            return Err(CoreError::InvalidTargetWeights(sum_weights));
        }
        if self.weights.etf_sp500_pct < dec!(0)
            || self.weights.nvda_pct < dec!(0)
            || self.weights.mp_pct < dec!(0)
        {
            return Err(CoreError::InvalidConfig(
                "I pesi target non possono essere negativi".to_string(),
            ));
        }
        if !(1..=28).contains(&self.pac.execution_day_of_month) {
            return Err(CoreError::InvalidPacDay(self.pac.execution_day_of_month));
        }
        if self.pac.phase1_months == 0 {
            return Err(CoreError::InvalidConfig(
                "La durata della Fase 1 deve essere almeno 1 mese".to_string(),
            ));
        }
        if self.pac.phase1_monthly_eur <= dec!(0) || self.pac.phase2_monthly_eur <= dec!(0) {
            return Err(CoreError::InvalidConfig(
                "Le rate del PAC devono essere strettamente positive".to_string(),
            ));
        }
        if self.profile.emergency_fund_eur < dec!(0) {
            return Err(CoreError::NegativeValue {
                field: "emergency_fund_eur",
                value: self.profile.emergency_fund_eur,
            });
        }
        if self.profile.initial_tr_cash_eur < dec!(0) {
            return Err(CoreError::NegativeValue {
                field: "initial_tr_cash_eur",
                value: self.profile.initial_tr_cash_eur,
            });
        }
        let t = &self.thresholds;
        if !(t.etf_dd_yellow_min_pct < t.etf_dd_orange_min_pct
            && t.etf_dd_orange_min_pct < t.etf_dd_red_min_pct)
        {
            return Err(CoreError::InvalidConfig(
                "Le soglie di drawdown ETF devono essere strettamente crescenti".to_string(),
            ));
        }
        if !(t.etf_advance_yellow_max_pct <= t.etf_advance_orange_cum_max_pct
            && t.etf_advance_orange_cum_max_pct <= t.etf_advance_red_cum_max_pct)
        {
            return Err(CoreError::InvalidConfig(
                "I tetti di anticipo ETF devono essere crescenti".to_string(),
            ));
        }
        if t.nvda_dd_yellow_pct >= t.nvda_dd_red_pct {
            return Err(CoreError::InvalidConfig(
                "Su NVDA il drawdown Giallo deve essere inferiore al drawdown Rosso".to_string(),
            ));
        }
        if t.hysteresis_pct < dec!(0) {
            return Err(CoreError::NegativeValue {
                field: "hysteresis_pct",
                value: t.hysteresis_pct,
            });
        }
        Ok(())
    }

    pub fn from_toml_str(raw: &str) -> Result<Self, CoreError> {
        let cfg: Self = toml::from_str(raw).map_err(|e| CoreError::TomlParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn to_toml_string(&self) -> Result<String, CoreError> {
        toml::to_string_pretty(self).map_err(|e| CoreError::TomlParse(e.to_string()))
    }
}