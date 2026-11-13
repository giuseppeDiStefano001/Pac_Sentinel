use crate::secrets;
use chrono::NaiveDate;
use pac_core::config::PortfolioConfig;
use pac_core::pac::{covered_phase1_months, remaining_tr_liquidity};
use pac_core::portfolio::{evaluate_emergency_fund, evaluate_portfolio, PortfolioValuation};
use pac_core::signals::{evaluate_signals, PortfolioSignalsReport, QualitativeContext};
use pac_core::tax::{simulate_hypothetical_sale, HypotheticalSaleSimulation};
use pac_core::types::{AssetId, BuyOperation, Currency, EmergencyFundStatus, MarketSnapshot};
use pac_data::db::{JournalEntry, PortfolioRepository, ThesisState};
use pac_data::market_hours::now_rome;
use pac_data::providers::api_key::{ApiKeyMarketProvider, ApiProviderKind};
use pac_data::providers::bce::BceFxClient;
use pac_data::providers::stooq::StooqProvider;
use pac_data::providers::yahoo::YahooFinanceProvider;
use pac_data::providers::{FallbackPriceChain, PriceProvider};
use pac_data::service::MarketDataService;
use pac_notify::client::TelegramBotClient;
use pac_notify::formatter::format_welcome_message;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiTab {
    Panoramica,
    PianoPac,
    Operazioni,
    Simulatore,
    TesiDiario,
    Impostazioni,
}

#[derive(Debug)]
pub enum BackgroundEvent {
    MarketRefreshed(Result<String, String>),
    TelegramTestSent(Result<String, String>),
}

pub struct NewOperationForm {
    pub asset: AssetId,
    pub date_str: String,
    pub invested_eur_str: String,
    pub unit_price_str: String,
    pub eur_usd_str: String,
    pub is_extra_advance: bool,
    pub note_str: String,
}

pub struct SaleSimForm {
    pub asset: AssetId,
    pub qty_to_sell_str: String,
    pub sale_price_eur_str: String,
    pub last_result: Option<HypotheticalSaleSimulation>,
}

pub struct SettingsForm {
    pub config_toml_draft: String,
    pub telegram_token_input: String,
    pub market_api_key_input: String,
    pub manual_symbol_input: String,
    pub manual_price_input: String,
}

pub struct PacSentinelGuiApp {
    pub config_path: PathBuf,
    pub db_path: PathBuf,
    pub config: PortfolioConfig,
    pub active_tab: GuiTab,

    pub current_pac_month: u32,
    pub eur_usd_rate: Decimal,
    pub fx_date_label: String,
    pub snapshots: BTreeMap<AssetId, MarketSnapshot>,
    pub operations: Vec<BuyOperation>,
    pub valuation: PortfolioValuation,
    pub report: PortfolioSignalsReport,
    pub ef_status: EmergencyFundStatus,
    pub tr_cash_remaining: Decimal,
    pub covered_months: u32,

    pub thesis_state: ThesisState,
    pub journal_entries: Vec<JournalEntry>,
    pub new_journal_asset: Option<AssetId>,
    pub new_journal_title: String,
    pub new_journal_body: String,

    pub nvda_earnings_str: String,
    pub mp_earnings_str: String,

    pub new_op_form: NewOperationForm,
    pub sale_sim_form: SaleSimForm,
    pub settings_form: SettingsForm,

    pub sim_horizon_years: usize,
    pub sim_cautious_pct: String,
    pub sim_base_pct: String,
    pub sim_optimistic_pct: String,

    pub status_banner: String,
    pub is_busy: bool,
    pub bg_tx: Sender<BackgroundEvent>,
    pub bg_rx: Receiver<BackgroundEvent>,
    pub rt_handle: tokio::runtime::Handle,
}

impl PacSentinelGuiApp {
    pub fn new(
        config_path: PathBuf,
        db_path: PathBuf,
        config: PortfolioConfig,
        rt_handle: tokio::runtime::Handle,
    ) -> anyhow::Result<Self> {
        let (bg_tx, bg_rx) = mpsc::channel();
        let today = now_rome().date_naive();
        let toml_str = config.to_toml_string()?;

        let repo = PortfolioRepository::open_file(&db_path)?;
        let current_pac_month = repo.get_current_pac_month()?;
        let thesis_state = repo.load_thesis_state()?;

        let mut nvda_earnings_str = String::new();
        let mut mp_earnings_str = String::new();
        for (asset, d) in repo.list_earnings_dates()? {
            match asset {
                AssetId::Nvda => nvda_earnings_str = d.to_string(),
                AssetId::Mp => mp_earnings_str = d.to_string(),
                AssetId::EtfSp500 => {}
            }
        }

        let dummy_val = evaluate_portfolio(&config, &[], &BTreeMap::new(), 0)?;
        let dummy_rep = evaluate_signals(
            &config,
            &dummy_val,
            &BTreeMap::new(),
            &QualitativeContext::default(),
        )?;
        let ef_status = evaluate_emergency_fund(&config);

        let mut app = Self {
            config_path,
            db_path,
            config,
            active_tab: GuiTab::Panoramica,
            current_pac_month,
            eur_usd_rate: dec!(1.10),
            fx_date_label: "N/D".to_string(),
            snapshots: BTreeMap::new(),
            operations: Vec::new(),
            valuation: dummy_val,
            report: dummy_rep,
            ef_status,
            tr_cash_remaining: dec!(6000),
            covered_months: 24,
            thesis_state,
            journal_entries: Vec::new(),
            new_journal_asset: None,
            new_journal_title: String::new(),
            new_journal_body: String::new(),
            nvda_earnings_str,
            mp_earnings_str,
            new_op_form: NewOperationForm {
                asset: AssetId::EtfSp500,
                date_str: today.to_string(),
                invested_eur_str: "200.00".to_string(),
                unit_price_str: String::new(),
                eur_usd_str: "1.10".to_string(),
                is_extra_advance: false,
                note_str: "Rata PAC mensile".to_string(),
            },
            sale_sim_form: SaleSimForm {
                asset: AssetId::Nvda,
                qty_to_sell_str: "1.0".to_string(),
                sale_price_eur_str: "150.00".to_string(),
                last_result: None,
            },
            settings_form: SettingsForm {
                config_toml_draft: toml_str,
                telegram_token_input: String::new(),
                market_api_key_input: String::new(),
                manual_symbol_input: "CSSPX.MI".to_string(),
                manual_price_input: String::new(),
            },
            sim_horizon_years: 10,
            sim_cautious_pct: "5.0".to_string(),
            sim_base_pct: "8.0".to_string(),
            sim_optimistic_pct: "11.0".to_string(),
            status_banner: "Pronto. Premi 'Aggiorna quotazioni ora' per scaricare gli ultimi prezzi."
                .to_string(),
            is_busy: false,
            bg_tx,
            bg_rx,
            rt_handle,
        };

        app.reload_from_db()?;
        app.trigger_market_refresh();
        Ok(app)
    }

    pub fn reload_from_db(&mut self) -> anyhow::Result<()> {
        let repo = PortfolioRepository::open_file(&self.db_path)?;
        let now = now_rome();

        if let Some((d, rate, src)) = repo.latest_eur_usd()? {
            self.eur_usd_rate = rate;
            self.fx_date_label = format!("{rate} ({src} {d})");
            self.new_op_form.eur_usd_str = rate.to_string();
        }

        self.snapshots = repo.build_all_snapshots(
            &self.config.market.etf_symbol,
            self.eur_usd_rate,
            now,
            self.config.market.stale_after_hours,
        )?;
        self.operations = repo.list_operations()?;
        self.journal_entries = repo.list_journal_entries()?;
        self.thesis_state = repo.load_thesis_state()?;

        let completed_months = self.current_pac_month.saturating_sub(1);
        let extra_advances = repo.sum_extra_etf_advances_eur()?;
        self.tr_cash_remaining = remaining_tr_liquidity(
            &self.config,
            completed_months.min(self.config.pac.phase1_months),
            extra_advances,
        )?;
        self.covered_months = covered_phase1_months(&self.config, self.tr_cash_remaining);
        self.ef_status = evaluate_emergency_fund(&self.config);

        self.valuation = evaluate_portfolio(
            &self.config,
            &self.operations,
            &self.snapshots,
            completed_months,
        )?;

        let ctx = QualitativeContext {
            current_pac_month: self.current_pac_month,
            remaining_tr_cash_eur: self.tr_cash_remaining,
            evaluation_date: Some(now.date_naive()),
            nvda_thesis_confirmed: self.thesis_state.nvda_confirmed,
            mp_thesis_compromised: self.thesis_state.mp_compromised,
            ..QualitativeContext::default()
        };

        self.report = evaluate_signals(&self.config, &self.valuation, &self.snapshots, &ctx)?;
        Ok(())
    }

    pub fn trigger_market_refresh(&mut self) {
        if self.is_busy {
            return;
        }
        self.is_busy = true;
        self.status_banner = "Aggiornamento quotazioni e cambio BCE in corso...".to_string();

        let db_path = self.db_path.clone();
        let cfg = self.config.clone();
        let tx = self.bg_tx.clone();

        // Eseguiamo su thread blocking dedicato con runtime single-thread così `rusqlite::Connection`
        // resta confinata su un solo thread e non deve implementare `Send`/`Sync`.
        self.rt_handle.spawn_blocking(move || {
            let res = (|| -> anyhow::Result<String> {
                let local_rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                local_rt.block_on(async move {
                    let repo = PortfolioRepository::open_file(&db_path)?;
                    let http = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .build()?;
                    let yahoo = YahooFinanceProvider::new("PacSentinel/0.1 (DesktopGUI)", 10, 2)?;
                    let stooq = StooqProvider::new(http.clone());
                    let mut providers: Vec<Box<dyn PriceProvider>> =
                        vec![Box::new(yahoo), Box::new(stooq)];

                    if let Some(api_key) = secrets::load_secret("market_api_key") {
                        let kind = if cfg.market.api_provider.eq_ignore_ascii_case("finnhub") {
                            ApiProviderKind::Finnhub
                        } else {
                            ApiProviderKind::TwelveData
                        };
                        providers.push(Box::new(ApiKeyMarketProvider::new(
                            http.clone(),
                            kind,
                            api_key,
                        )));
                    }

                    let chain = FallbackPriceChain::new(providers);
                    let bce = BceFxClient::new(http);
                    let service = MarketDataService::new(&repo, &chain, &bce);
                    let snaps = service.refresh_all_market_data(&cfg).await?;
                    Ok(format!(
                        "Aggiornamento completato ({} strumenti sincronizzati alle {}).",
                        snaps.len(),
                        now_rome().format("%H:%M:%S")
                    ))
                })
            })()
            .map_err(|e| e.to_string());

            let _ = tx.send(BackgroundEvent::MarketRefreshed(res));
        });
    }

    pub fn trigger_telegram_test(&mut self) {
        let Some(chat_id) = self.config.notifications.authorized_chat_id else {
            self.status_banner =
                "Configura prima 'authorized_chat_id' nei parametri o tramite `app telegram-setup`."
                    .to_string();
            return;
        };
        let Some(token) = secrets::load_secret("telegram_bot_token") else {
            self.status_banner =
                "Token Telegram mancante nel keyring. Inseriscilo nelle Impostazioni.".to_string();
            return;
        };

        self.status_banner = "Invio messaggio di prova su Telegram...".to_string();
        let tx = self.bg_tx.clone();
        self.rt_handle.spawn(async move {
            let res = async {
                let client = TelegramBotClient::new(token)?;
                client
                    .send_html_message(chat_id, &format_welcome_message())
                    .await?;
                Ok::<String, anyhow::Error>("Messaggio di prova inviato su Telegram.".to_string())
            }
            .await
            .map_err(|e| e.to_string());

            let _ = tx.send(BackgroundEvent::TelegramTestSent(res));
        });
    }

    pub fn insert_operation_from_form(&mut self) -> anyhow::Result<()> {
        let date = NaiveDate::parse_from_str(self.new_op_form.date_str.trim(), "%Y-%m-%d")?;
        let invested_eur = Decimal::from_str(self.new_op_form.invested_eur_str.trim())?;
        let unit_price_native = Decimal::from_str(self.new_op_form.unit_price_str.trim())?;
        let eur_usd_rate = Decimal::from_str(self.new_op_form.eur_usd_str.trim())?;

        if invested_eur <= dec!(0) || unit_price_native <= dec!(0) || eur_usd_rate <= dec!(0) {
            anyhow::bail!("Importo, prezzo e cambio devono essere strettamente positivi.");
        }

        let native_currency = match self.new_op_form.asset {
            AssetId::EtfSp500 => Currency::Eur,
            AssetId::Nvda | AssetId::Mp => Currency::Usd,
        };

        let invested_native = match native_currency {
            Currency::Eur => invested_eur,
            Currency::Usd => invested_eur * eur_usd_rate,
        };
        let quantity = (invested_native / unit_price_native).round_dp(6);

        let op = BuyOperation {
            id: None,
            asset: self.new_op_form.asset,
            date,
            invested_eur,
            unit_price_native,
            native_currency,
            eur_usd_rate,
            quantity,
            note: if self.new_op_form.note_str.trim().is_empty() {
                None
            } else {
                Some(self.new_op_form.note_str.trim().to_string())
            },
        };

        let repo = PortfolioRepository::open_file(&self.db_path)?;
        repo.insert_operation(&op, self.new_op_form.is_extra_advance)?;
        self.reload_from_db()?;
        self.status_banner = format!(
            "Operazione registrata: {} ({:.2} EUR -> {:.6} quote).",
            op.asset.code(),
            op.invested_eur,
            op.quantity
        );
        Ok(())
    }

    pub fn run_sale_simulation(&mut self) -> anyhow::Result<()> {
        let qty_to_sell = Decimal::from_str(self.sale_sim_form.qty_to_sell_str.trim())?;
        let sale_price_eur = Decimal::from_str(self.sale_sim_form.sale_price_eur_str.trim())?;
        let pos = &self.valuation.positions[&self.sale_sim_form.asset];

        let sim = simulate_hypothetical_sale(
            pos.total_quantity,
            pos.average_cost_eur,
            qty_to_sell,
            sale_price_eur,
            self.valuation.total_market_value_eur,
            &self.config,
        )?;
        self.sale_sim_form.last_result = Some(sim);
        Ok(())
    }
}

pub fn dec_to_f64(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}