use crate::error::NotifyError;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

/// Client HTTP asincrono per Telegram Bot API (senza dipendenze pesanti, solo `reqwest` + `rustls`).
/// Non logga mai il token del bot né il `chat_id`.
pub struct TelegramBotClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    max_retries: u32,
}

impl TelegramBotClient {
    pub fn new(token: String) -> Result<Self, NotifyError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(35))
            .build()?;
        Ok(Self {
            http,
            base_url: "https://api.telegram.org".to_string(),
            token,
            max_retries: 4,
        })
    }

    pub fn with_base_url(token: String, base_url: String, max_retries: u32) -> Result<Self, NotifyError> {
        let mut c = Self::new(token)?;
        c.base_url = base_url;
        c.max_retries = max_retries;
        Ok(c)
    }

    fn endpoint(&self, method_name: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.base_url.trim_end_matches('/'),
            self.token,
            method_name
        )
    }

    /// Invia un messaggio HTML con retry esponenziale e rispetto automatico di `HTTP 429 retry_after`.
    pub async fn send_html_message(&self, chat_id: i64, html_text: &str) -> Result<(), NotifyError> {
        let url = self.endpoint("sendMessage");
        let payload = SendMessageRequest {
            chat_id,
            text: html_text,
            parse_mode: "HTML",
            disable_web_page_preview: true,
        };

        let mut attempt = 0_u32;
        loop {
            let response = self.http.post(&url).json(&payload).send().await;
            match response {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(());
                    }

                    let body_text = resp.text().await.unwrap_or_default();
                    let parsed_err: Option<TelegramErrorResponse> =
                        serde_json::from_str(&body_text).ok();

                    if status.as_u16() == 429 && attempt < self.max_retries {
                        let wait_secs = parsed_err
                            .as_ref()
                            .and_then(|e| e.parameters.as_ref())
                            .and_then(|p| p.retry_after)
                            .unwrap_or(2)
                            .min(60);
                        warn!(wait_secs = wait_secs, "Rate limit Telegram (429), attendo retry_after");
                        tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                        attempt += 1;
                        continue;
                    }

                    if status.is_server_error() && attempt < self.max_retries {
                        attempt += 1;
                        let backoff_ms = 300_u64 * (1_u64 << attempt.min(5));
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        continue;
                    }

                    let desc = parsed_err
                        .and_then(|e| e.description)
                        .unwrap_or(body_text);
                    return Err(NotifyError::TelegramApi {
                        status: status.as_u16(),
                        description: desc,
                    });
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(NotifyError::Http(e));
                    }
                    attempt += 1;
                    let backoff_ms = 300_u64 * (1_u64 << attempt.min(5));
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }

    /// Recupera gli aggiornamenti in ingresso (usato sia per la configurazione guidata di `chat_id`
    /// sia per il polling dei comandi del bot).
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: u64,
    ) -> Result<Vec<TelegramUpdate>, NotifyError> {
        let mut url = format!("{}?timeout={timeout_secs}", self.endpoint("getUpdates"));
        if let Some(off) = offset {
            use std::fmt::Write;
            let _ = write!(url, "&offset={off}");
        }

        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let desc = resp.text().await.unwrap_or_default();
            return Err(NotifyError::TelegramApi {
                status: status.as_u16(),
                description: desc,
            });
        }

        let parsed: GetUpdatesResponse = resp.json().await?;
        Ok(parsed.result)
    }
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
    parse_mode: &'a str,
    disable_web_page_preview: bool,
}

#[derive(Debug, Deserialize)]
struct TelegramErrorResponse {
    description: Option<String>,
    parameters: Option<TelegramRetryParameters>,
}

#[derive(Debug, Deserialize)]
struct TelegramRetryParameters {
    retry_after: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    result: Vec<TelegramUpdate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    pub chat: TelegramChat,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}