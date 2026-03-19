use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::llm::ChatMessage;
use crate::memory::MemoryManager;
use crate::react::Agent;

#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    pub listen: SocketAddr,
    pub path: String,
    pub webhook_url: String,
    pub webhook_secret: Option<String>,
}

#[derive(Clone)]
struct AppState {
    agent: Arc<Mutex<Agent>>,
    memory: Arc<Mutex<MemoryManager>>,
    webhook: DingTalkWebhook,
    saved_len: Arc<Mutex<usize>>,
}

pub async fn run_server(cfg: DingTalkConfig, agent: Arc<Mutex<Agent>>, memory: Arc<Mutex<MemoryManager>>) -> Result<()> {
    let saved_len = {
        let a = agent.lock().await;
        a.messages().len()
    };
    let state = AppState {
        agent,
        memory,
        webhook: DingTalkWebhook::new(cfg.webhook_url, cfg.webhook_secret),
        saved_len: Arc::new(Mutex::new(saved_len)),
    };

    let path = cfg.path.clone();
    let app = Router::new()
        .route(&path, post(handle_callback))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("绑定钉钉监听地址失败: {}", cfg.listen))?;
    axum::serve(listener, app).await.context("运行钉钉HTTP服务失败")?;
    Ok(())
}

async fn handle_callback(
    State(state): State<AppState>,
    Json(body): Json<DingTalkCallback>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(challenge) = body.challenge {
        return Ok(Json(serde_json::json!({ "challenge": challenge })));
    }

    let text = body
        .text
        .and_then(|t| t.content)
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return Ok(Json(serde_json::json!({ "ok": true })));
    }

    let reply = {
        let mut agent = state.agent.lock().await;
        agent.push_user_text(text);
        match agent.run_turn().await {
            Ok(s) => s,
            Err(e) => format!("执行失败: {e}"),
        }
    };

    {
        let mut saved_len = state.saved_len.lock().await;
        let new_msgs: Vec<ChatMessage> = {
            let agent = state.agent.lock().await;
            agent.messages()[*saved_len..].to_vec()
        };
        if !new_msgs.is_empty() {
            let mut mem = state.memory.lock().await;
            let _ = mem.append_messages(&new_msgs);
            *saved_len += new_msgs.len();
        }
    }

    let _ = state.webhook.send_text(&reply).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
struct DingTalkCallback {
    #[serde(default)]
    challenge: Option<String>,
    #[serde(default)]
    text: Option<DingTalkText>,
}

#[derive(Debug, Deserialize)]
struct DingTalkText {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Clone)]
struct DingTalkWebhook {
    url: String,
    secret: Option<String>,
    http: reqwest::Client,
}

impl DingTalkWebhook {
    fn new(url: String, secret: Option<String>) -> Self {
        Self {
            url,
            secret,
            http: reqwest::Client::new(),
        }
    }

    async fn send_text(&self, content: &str) -> Result<()> {
        let url = self.signed_url()?;
        let body = serde_json::json!({
            "msgtype": "text",
            "text": { "content": content }
        });
        self.http.post(url).json(&body).send().await?;
        Ok(())
    }

    fn signed_url(&self) -> Result<String> {
        if let Some(secret) = &self.secret {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let string_to_sign = format!("{ts}\n{secret}");
            let sign = hmac_sha256_base64(secret.as_bytes(), string_to_sign.as_bytes());
            let mut url = reqwest::Url::parse(&self.url).context("钉钉webhook_url不是合法URL")?;
            url.query_pairs_mut()
                .append_pair("timestamp", &ts.to_string())
                .append_pair("sign", &sign);
            Ok(url.to_string())
        } else {
            Ok(self.url.clone())
        }
    }
}

fn hmac_sha256_base64(key: &[u8], data: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    mac.update(data);
    let bytes = mac.finalize().into_bytes();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
