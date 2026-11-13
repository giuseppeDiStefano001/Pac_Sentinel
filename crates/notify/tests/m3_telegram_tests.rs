use chrono::TimeZone;
use chrono_tz::Europe::Rome;
use pac_core::config::PortfolioConfig;
use pac_core::types::{AssetId, Currency, SignalLevel};
use pac_data::db::{LatestQuoteRecord, PortfolioRepository};
use pac_notify::client::TelegramBotClient;
use pac_notify::commands::{handle_authorized_command, parse_bot_command, BotCommand};
use pac_notify::formatter::escape_html;
use pac_notify::policy::{is_quiet_hours, NotificationGatekeeper};
use rust_decimal_macros::dec;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn test_html_escaping_prevents_injection() {
    let raw = "NVDA < 150 & > 100 \"test\"";
    let escaped = escape_html(raw);
    assert_eq!(escaped, "NVDA &lt; 150 &amp; &gt; 100 &quot;test&quot;");
}

#[test]
fn test_antispam_cooldown_quiet_hours_and_mute() {
    let repo = PortfolioRepository::open_in_memory().unwrap();
    let cfg = PortfolioConfig::default();
    let gate = NotificationGatekeeper::new(&repo, &cfg.notifications);

    let t1 = Rome.with_ymd_and_hms(2026, 11, 13, 9, 0, 0).unwrap();

    // Primo rilevamento VERDE -> non notifica
    assert_eq!(
        gate.should_notify_asset_signal(AssetId::Nvda, SignalLevel::Verde, t1)
            .unwrap(),
        None
    );

    // Passaggio VERDE -> GIALLO -> notifica immediatamente!
    assert_eq!(
        gate.should_notify_asset_signal(AssetId::Nvda, SignalLevel::Giallo, t1)
            .unwrap(),
        Some(Some(SignalLevel::Verde))
    );

    // Dopo 2 ore ancora GIALLO -> soppresso dal cooldown (12h)
    let t2 = t1 + chrono::Duration::hours(2);
    assert_eq!(
        gate.should_notify_asset_signal(AssetId::Nvda, SignalLevel::Giallo, t2)
            .unwrap(),
        None
    );

    // Dopo 13 ore ancora GIALLO -> cooldown scaduto -> notifica
    let t3 = t1 + chrono::Duration::hours(13);
    assert_eq!(
        gate.should_notify_asset_signal(AssetId::Nvda, SignalLevel::Giallo, t3)
            .unwrap(),
        Some(Some(SignalLevel::Giallo))
    );

    // Ore silenziose (02:00): GIALLO bloccato, ROSSO passa
    let night = Rome.with_ymd_and_hms(2026, 11, 14, 2, 0, 0).unwrap();
    assert!(is_quiet_hours(night, &cfg.notifications));
    assert_eq!(
        gate.should_notify_asset_signal(AssetId::Mp, SignalLevel::Giallo, night)
            .unwrap(),
        None
    );
    assert_eq!(
        gate.should_notify_asset_signal(AssetId::Mp, SignalLevel::Rosso, night)
            .unwrap(),
        Some(None)
    );
}

#[test]
fn test_bot_readonly_commands_and_silenzio() {
    let repo = PortfolioRepository::open_in_memory().unwrap();
    let cfg = PortfolioConfig::default();
    let now = Rome.with_ymd_and_hms(2026, 11, 13, 16, 0, 0).unwrap();

    repo.upsert_fx_rate(now.date_naive(), dec!(1.10), "BCE")
        .unwrap();
    repo.upsert_latest_quote(&LatestQuoteRecord {
        symbol: "NVDA".to_string(),
        price_native: dec!(166.19),
        currency: Currency::Usd,
        source: "Yahoo Finance".to_string(),
        timestamp_rome: now,
    })
    .unwrap();

    assert_eq!(parse_bot_command("/stato"), BotCommand::Stato);
    assert_eq!(
        parse_bot_command("/silenzio 24h"),
        BotCommand::Silenzio { hours: Some(24) }
    );

    let pac_reply =
        handle_authorized_command(&BotCommand::Pac, &repo, &cfg, 1, now).unwrap();
    assert!(pac_reply.contains("250.00 EUR"));
    assert!(pac_reply.contains("non è consulenza finanziaria"));

    let mute_reply = handle_authorized_command(
        &BotCommand::Silenzio { hours: Some(24) },
        &repo,
        &cfg,
        1,
        now,
    )
    .unwrap();
    assert!(mute_reply.contains("24 ore"));

    let gate = NotificationGatekeeper::new(&repo, &cfg.notifications);
    assert!(gate.is_muted(now + chrono::Duration::hours(1)).unwrap());
}

#[tokio::test]
async fn test_telegram_client_handles_http_429_retry_after() {
    let mock_server = MockServer::start().await;

    // Prima chiamata risponde 429 con retry_after = 1
    Mock::given(method("POST"))
        .and(path("/botTEST_TOKEN/sendMessage"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string(r#"{"ok":false,"description":"Too Many Requests","parameters":{"retry_after":1}}"#),
        )
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Seconda chiamata risponde 200 OK
    Mock::given(method("POST"))
        .and(path("/botTEST_TOKEN/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&mock_server)
        .await;

    let client =
        TelegramBotClient::with_base_url("TEST_TOKEN".to_string(), mock_server.uri(), 2).unwrap();
    client
        .send_html_message(123_456_789, "<b>Test retry</b>")
        .await
        .expect("invio riuscito dopo retry_after");
}