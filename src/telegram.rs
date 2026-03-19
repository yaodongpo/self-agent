use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::TelegramConfig;
use crate::memory::MemoryManager;
use crate::react::Agent;

#[derive(Clone)]
struct TelegramClient {
    token: String,
    http: reqwest::Client,
}

impl TelegramClient {
    fn new(token: String) -> Self {
        Self {
            token,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.token, method)
    }

    async fn get_updates(&self, offset: i64, timeout_seconds: u64) -> Result<Vec<Update>> {
        let url = self.url("getUpdates");
        let res = self
            .http
            .get(url)
            .query(&[
                ("offset", offset.to_string()),
                ("timeout", timeout_seconds.to_string()),
            ])
            .send()
            .await
            .context("请求Telegram getUpdates失败")?;
        let status = res.status();
        let body = res.text().await.context("读取Telegram响应失败")?;
        if !status.is_success() {
            anyhow::bail!("Telegram getUpdates错误: status={status} body={body}");
        }
        let parsed = serde_json::from_str::<TelegramResponse<Vec<Update>>>(&body)
            .context("解析Telegram getUpdates响应失败")?;
        if !parsed.ok {
            anyhow::bail!("Telegram getUpdates返回ok=false");
        }
        Ok(parsed.result.unwrap_or_default())
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<()> {
        let url = self.url("sendMessage");
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true
        });
        let res = self.http.post(url).json(&body).send().await?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("Telegram sendMessage错误: status={status} body={body}");
        }
        Ok(())
    }
}

pub async fn run_bot(cfg: TelegramConfig, agent: Arc<Mutex<Agent>>, memory: Arc<Mutex<MemoryManager>>) -> Result<()> {
    let client = TelegramClient::new(cfg.token);
    let mut offset: i64 = 0;

    let saved_len = {
        let a = agent.lock().await;
        a.messages().len()
    };
    let saved_len = Arc::new(Mutex::new(saved_len));

    loop {
        let updates = match client.get_updates(offset, 30).await {
            Ok(u) => u,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(cfg.poll_interval_ms)).await;
                continue;
            }
        };

        for u in updates {
            offset = offset.max(u.update_id + 1);
            let Some(m) = u.message.or(u.edited_message) else {
                continue;
            };
            let chat_id = m.chat.id;
            if !cfg.allowed_chat_ids.is_empty() && !cfg.allowed_chat_ids.contains(&chat_id) {
                continue;
            }
            let Some(text) = m.text else {
                continue;
            };
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }

            let reply = {
                let mut a = agent.lock().await;
                a.push_user_text(text);
                match a.run_turn().await {
                    Ok(s) => s,
                    Err(e) => format!("执行失败: {e}"),
                }
            };

            {
                let mut saved = saved_len.lock().await;
                let new_msgs = {
                    let a = agent.lock().await;
                    a.messages()[*saved..].to_vec()
                };
                if !new_msgs.is_empty() {
                    let mut mem = memory.lock().await;
                    let _ = mem.append_messages(&new_msgs);
                    *saved += new_msgs.len();
                }
            }

            let _ = client.send_message(chat_id, &reply).await;
        }

        tokio::time::sleep(std::time::Duration::from_millis(cfg.poll_interval_ms)).await;
    }
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    edited_message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    chat: Chat,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
}
