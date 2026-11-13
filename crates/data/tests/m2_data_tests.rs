use chrono::{NaiveDate, TimeZone};
use chrono_tz::Europe::Rome;
use pac_core::config::PortfolioConfig;
use pac_core::types::{AssetId, Currency};
use pac_data::db::{DailyBar, PortfolioRepository};
use pac_data::market_hours::{is_price_stale, MarketVenue};
use pac_data::providers::bce::BceFxClient;
use pac_data::providers::stooq::StooqProvider;
use pac_data::providers::yahoo::YahooFinanceProvider;
use pac_data::providers::FallbackPriceChain;
use pac_data::service::MarketDataService;
use rust_decimal_macros::dec;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn test_sqlite_migrations_and_52w_high_calculation() {
    let repo = PortfolioRepository::open_in_memory().expect("apertura DB in-memory");
    assert_eq!(repo.schema_version().unwrap(), 2);

    let now = Rome.with_ymd_and_hms(2026, 5, 15, 16, 0, 0).unwrap();
    let base_date = NaiveDate::from_ymd_opt(2025, 4, 1).unwrap();

    // Inseriamo 400 giorni di storico con massimo a 600 EUR e chiusura 30g fa a 500 EUR
    let mut bars = Vec::new();
    for i in 0..400_i64 {
        let d = base_date + chrono::Duration::days(i);
        let price = if d == NaiveDate::from_ymd_opt(2026, 2, 10).unwrap() {
            dec!(600.00)
        } else if d <= NaiveDate::from_ymd_opt(2026, 4, 15).unwrap() {
            dec!(500.00)
        } else {
            dec!(520.00)
        };
        bars.push(DailyBar {
            date: d,
            close_native: price,
            currency: Currency::Eur,
            source: "test".to_string(),
        });
    }
    repo.upsert_daily_bars("CSSPX.MI", &bars, now).unwrap();
    assert_eq!(repo.history_count("CSSPX.MI").unwrap(), 400);

    // Prezzo spot corrente = 480 EUR -> drawdown dal massimo 600 EUR = (600-480)/600 = 20.00%
    repo.upsert_latest_quote(&pac_data::db::LatestQuoteRecord {
        symbol: "CSSPX.MI".to_string(),
        price_native: dec!(480.00),
        currency: Currency::Eur,
        source: "test".to_string(),
        timestamp_rome: now,
    })
    .unwrap();

    let snap = repo
        .build_market_snapshot(AssetId::EtfSp500, "CSSPX.MI", dec!(1.10), now, 4)
        .unwrap()
        .expect("snapshot presente");

    assert_eq!(snap.high_52w_native, dec!(600.00));
    assert_eq!(snap.drawdown_52w_pct(), dec!(20.00));
    assert_eq!(snap.change_30d_pct, dec!(-4.00)); // (480 - 500) / 500 = -4%
    assert!(!snap.is_stale);
}

#[test]
fn test_market_hours_and_staleness_rules() {
    // Mercoledì 16:00 Europe/Rome -> sia Borsa Italiana che NYSE aperte
    let open_ts = Rome.with_ymd_and_hms(2026, 5, 13, 16, 0, 0).unwrap();
    assert!(MarketVenue::BorsaItaliana.is_open_at(open_ts));
    assert!(MarketVenue::UsEquity.is_open_at(open_ts));

    // Dato vecchio di 5 ore durante mercato aperto -> stale = true (soglia 4h)
    let old_quote = open_ts - chrono::Duration::hours(5);
    assert!(is_price_stale(AssetId::Nvda, old_quote, open_ts, 4));

    // Sabato pomeriggio -> mercati chiusi, dato di venerdì sera (18 ore fa) NON è stale
    let sat_ts = Rome.with_ymd_and_hms(2026, 5, 16, 15, 0, 0).unwrap();
    let fri_close = Rome.with_ymd_and_hms(2026, 5, 15, 21, 55, 0).unwrap();
    assert!(!MarketVenue::UsEquity.is_open_at(sat_ts));
    assert!(!is_price_stale(AssetId::Nvda, fri_close, sat_ts, 4));
}

#[tokio::test]
async fn test_fallback_chain_yahoo_to_stooq_and_bce_xml() {
    let mock_server = MockServer::start().await;

    // 1. Feed BCE eurofxref simulato
    let bce_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01" xmlns="http://www.ecb.int/vocabulary/2002-08-01/eurofxref">
      <Cube>
        <Cube time="2026-05-15">
          <Cube currency="USD" rate="1.1250"/>
          <Cube currency="JPY" rate="165.40"/>
        </Cube>
      </Cube>
    </gesmes:Envelope>"#;

    Mock::given(method("GET"))
        .and(path("/eurofxref-daily.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(bce_xml))
        .mount(&mock_server)
        .await;

    // 2. Yahoo Finance risponde 503 (simula guasto) -> deve scattare il fallback su Stooq
    Mock::given(method("GET"))
        .and(path("/v8/finance/chart/NVDA"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&mock_server)
        .await;

    // 3. Stooq risponde con CSV valido
    let stooq_csv = "Date,Open,High,Low,Close,Volume\n2026-05-14,130.0,132.0,129.0,131.50,1000000\n2026-05-15,131.5,136.0,131.0,135.00,1200000\n";
    Mock::given(method("GET"))
        .and(path("/q/d/l/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(stooq_csv))
        .mount(&mock_server)
        .await;

    let http = reqwest::Client::new();
    let bce = BceFxClient::with_custom_url(
        http.clone(),
        format!("{}/eurofxref-daily.xml", mock_server.uri()),
    );
    let yahoo = YahooFinanceProvider::with_base_url(
        "PacSentinelTest/1.0",
        mock_server.uri(),
        2,
        0,
    )
    .unwrap();
    let stooq = StooqProvider::with_base_url(http, mock_server.uri());

    let chain = FallbackPriceChain::new(vec![Box::new(yahoo), Box::new(stooq)]);
    let repo = PortfolioRepository::open_in_memory().unwrap();
    let service = MarketDataService::new(&repo, &chain, &bce);

    let fx = service.sync_eur_usd_rate().await.unwrap();
    assert_eq!(fx, dec!(1.1250));

    let quote = chain
        .fetch_latest_with_fallback("NVDA", Currency::Usd)
        .await
        .expect("fallback su Stooq riuscito");
    assert_eq!(quote.price_native, dec!(135.00));
    assert_eq!(quote.source, "Stooq CSV");

    // Verifica conversione EUR/USD: 135.00 / 1.1250 = 120.00 EUR
    let _ = PortfolioConfig::default();
}