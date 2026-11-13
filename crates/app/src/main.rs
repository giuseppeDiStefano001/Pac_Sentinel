mod daemon;
mod gui;
mod secrets;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use pac_core::config::PortfolioConfig;
use pac_core::pac::remaining_tr_liquidity;
use pac_core::portfolio::{evaluate_emergency_fund, evaluate_portfolio};
use pac_core::signals::{evaluate_signals, QualitativeContext};
use pac_core::types::{AssetId, Currency};
use pac_core::LEGAL_DISCLAIMER;
use pac_data::db::PortfolioRepository;
use pac_data::market_hours::now_rome;
use pac_data::providers::api_key::{ApiKeyMarketProvider, ApiProviderKind};
use pac_data::providers::bce::BceFxClient;
use pac_data::providers::stooq::StooqProvider;
use pac_data::providers::yahoo::YahooFinanceProvider;
use pac_data::providers::{FallbackPriceChain, PriceProvider};
use pac_data::service::MarketDataService;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::path::PathBuf;
use std::str::FromStr;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "app", about = "PAC Sentinel - Gestione portafoglio a lungo termine")]
struct Cli {
    /// Avvia l'applicazione in modalità daemon senza finestra (notifiche Telegram e polling mercati)
    #[arg(long)]
    daemon: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Avvia l'interfaccia grafica desktop (default se non vengono passati sottocomandi)
    Gui,
    /// Aggiorna e mostra le quotazioni correnti, il cambio EUR/USD BCE e lo stato dati
    Prices {
        #[arg(long)]
        manual_symbol: Option<String>,
        #[arg(long)]
        manual_price: Option<String>,
    },
    /// Valuta il portafoglio e mostra i segnali semaforo e le azioni PAC suggerite
    Signals,
    /// Configurazione guidata del bot Telegram (token nel keyring e rilevamento `chat_id`)
    TelegramSetup,
    /// Salva una chiave API nel portachiavi nativo del sistema operativo (`keyring`)
    SetApiKey {
        /// Nome segreto (es. `market_api_key` o `telegram_bot_token`)
        name: String,
        value: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let (config_path, db_path) = resolve_app_paths()?;
    let mut config = load_or_create_config(&config_path)?;
    let repo = PortfolioRepository::open_file(&db_path)
        .with_context(|| format!("Errore apertura database SQLite in {}", db_path.display()))?;

    if cli.daemon {
        let chain = build_provider_chain(&config)?;
        return daemon::run_daemon_loop(&repo, &config, &chain).await;
    }

    match cli.command {
        Some(Commands::Gui) | None => {
            drop(repo);
            let handle = tokio::runtime::Handle::current();
            gui::launch_desktop_gui(config_path, db_path, config, handle)?;
        }
        Some(Commands::TelegramSetup) => {
            daemon::run_telegram_setup(&config_path, &mut config).await?;
        }
        Some(Commands::SetApiKey { name, value }) => {
            secrets::store_secret(&name, &value)?;
            println!("Segreto '{name}' salvato in sicurezza nel portachiavi di sistema.");
        }
        Some(Commands::Prices {
            manual_symbol,
            manual_price,
        }) => {
            run_prices_command(&repo, &config, manual_symbol, manual_price).await?;
        }
        Some(Commands::Signals) => {
            run_signals_command(&repo, &config).await?;
        }
    }

    Ok(())
}

fn resolve_app_paths() -> Result<(PathBuf, PathBuf)> {
    let dirs = ProjectDirs::from("it", "PacSentinel", "PacSentinel")
        .context("Impossibile determinare le directory standard del sistema operativo")?;
    let config_dir = dirs.config_dir();
    let data_dir = dirs.data_dir();
    std::fs::create_dir_all(config_dir)?;
    std::fs::create_dir_all(data_dir)?;
    Ok((
        config_dir.join("config.toml"),
        data_dir.join("portfolio.sqlite"),
    ))
}

fn load_or_create_config(path: &PathBuf) -> Result<PortfolioConfig> {
    if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        let cfg = PortfolioConfig::from_toml_str(&raw)?;
        Ok(cfg)
    } else {
        let cfg = PortfolioConfig::default();
        let toml_str = cfg.to_toml_string()?;
        std::fs::write(path, toml_str)?;
        Ok(cfg)
    }
}

fn build_provider_chain(config: &PortfolioConfig) -> Result<FallbackPriceChain> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let yahoo = YahooFinanceProvider::new("PacSentinel/0.1 (LongTermPortfolio)", 10, 2)?;
    let stooq = StooqProvider::new(http.clone());

    let mut providers: Vec<Box<dyn PriceProvider>> = vec![Box::new(yahoo), Box::new(stooq)];

    if let Some(api_key) = secrets::load_secret("market_api_key") {
        let kind = if config.market.api_provider.eq_ignore_ascii_case("finnhub") {
            ApiProviderKind::Finnhub
        } else {
            ApiProviderKind::TwelveData
        };
        providers.push(Box::new(ApiKeyMarketProvider::new(http, kind, api_key)));
    }

    Ok(FallbackPriceChain::new(providers))
}

async fn run_prices_command(
    repo: &PortfolioRepository,
    config: &PortfolioConfig,
    manual_symbol: Option<String>,
    manual_price: Option<String>,
) -> Result<()> {
    let http = reqwest::Client::new();
    let bce = BceFxClient::new(http);
    let chain = build_provider_chain(config)?;
    let service = MarketDataService::new(repo, &chain, &bce);

    if let (Some(sym), Some(price_str)) = (manual_symbol, manual_price) {
        let price = Decimal::from_str(&price_str).context("Prezzo manuale non valido")?;
        let currency = if sym.eq_ignore_ascii_case(&config.market.etf_symbol) {
            Currency::Eur
        } else {
            Currency::Usd
        };
        service.record_manual_price(&sym, price, currency)?;
        println!("Prezzo manuale registrato per {sym}: {price} {currency}");
    }

    let snapshots = service.refresh_all_market_data(config).await?;
    let fx_info = repo.latest_eur_usd()?;

    println!("=== QUOTAZIONI DI RIFERIMENTO ===");
    if let Some((fx_date, rate, src)) = fx_info {
        println!("Cambio EUR/USD ({src} del {fx_date}): {rate}");
    }
    println!(
        "Nota: i prezzi delle fonti pubbliche sono di riferimento e possono differire dall'esecuzione su Trade Republic.\n"
    );

    for asset in AssetId::ALL {
        if let Some(s) = snapshots.get(&asset) {
            let stale_tag = if s.is_stale { " [DATI VECCHI]" } else { "" };
            println!(
                "- {} ({}): {:.2} {} (~{:.2} EUR) | Var 7g: {:+.2}% | Var 30g: {:+.2}% | Max 52w: {:.2} | DD 52w: {:.2}% | Fonte: {} ({}){}",
                s.asset.display_name(),
                s.asset.code(),
                s.price_native,
                s.currency,
                s.price_eur,
                s.change_7d_pct,
                s.change_30d_pct,
                s.high_52w_native,
                s.drawdown_52w_pct(),
                s.source,
                s.timestamp_rome.format("%Y-%m-%d %H:%M %Z"),
                stale_tag
            );
        } else {
            println!("- {}: nessun dato disponibile", asset.display_name());
        }
    }

    println!("\n{LEGAL_DISCLAIMER}");
    Ok(())
}

async fn run_signals_command(repo: &PortfolioRepository, config: &PortfolioConfig) -> Result<()> {
    let http = reqwest::Client::new();
    let bce = BceFxClient::new(http);
    let chain = build_provider_chain(config)?;
    let service = MarketDataService::new(repo, &chain, &bce);

    let snapshots = service.refresh_all_market_data(config).await?;
    let operations = repo.list_operations()?;
    let valuation = evaluate_portfolio(config, &operations, &snapshots, 0)?;
    let ef_status = evaluate_emergency_fund(config);
    let tr_cash = remaining_tr_liquidity(config, 0, dec!(0))?;

    let ctx = QualitativeContext {
        current_pac_month: 1,
        remaining_tr_cash_eur: tr_cash,
        evaluation_date: Some(now_rome().date_naive()),
        nvda_thesis_confirmed: true,
        mp_thesis_compromised: false,
        ..QualitativeContext::default()
    };

    let report = evaluate_signals(config, &valuation, &snapshots, &ctx)?;

    println!("=== SEGNALI PORTAFOGLIO & PIANO PAC ===");
    println!("Stato generale semaforo: {}", report.overall_level);
    println!(
        "Liquidità Trade Republic residua: {:.2} EUR | Fondo emergenze (intoccabile): {:.2} EUR",
        tr_cash, ef_status.balance_eur
    );
    if ef_status.is_above_warning_threshold {
        println!(
            "ATTENZIONE BOLLO: il fondo emergenze supera {:.2} EUR (soglia imposta di bollo {:.2} EUR).",
            ef_status.stamp_duty_warning_threshold_eur, ef_status.stamp_duty_legal_threshold_eur
        );
    }

    println!("\n--- Dettaglio Strumenti ---");
    for asset in AssetId::ALL {
        let sig = &report.asset_signals[&asset];
        let pos = &valuation.positions[&asset];
        println!(
            "[{}] {} -> Peso reale: {:.2}% (atteso {:.2}%, target {:.2}%)\n  Regola: {}\n  Azione PAC: {}",
            sig.level,
            asset.display_name(),
            pos.actual_weight_pct,
            pos.expected_weight_pct,
            pos.target_weight_pct,
            sig.triggered_rule,
            sig.suggested_pac_action
        );
    }

    for alert in &report.portfolio_alerts {
        println!("\n! {alert}");
    }

    println!(
        "\nRata PAC suggerita (mese {}): Totale {:.2} € -> ETF {:.2} € | NVDA {:.2} € | MP {:.2} €",
        report.adjusted_pac_allocation.month_number,
        report.adjusted_pac_allocation.total_eur,
        report.adjusted_pac_allocation.etf_sp500_eur,
        report.adjusted_pac_allocation.nvda_eur,
        report.adjusted_pac_allocation.mp_eur
    );
    println!("{}", report.emergency_fund_reminder);
    println!("\n{LEGAL_DISCLAIMER}");
    Ok(())
}