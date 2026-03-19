use anyhow::{Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    OpenAiChatCompletions,
    ArkResponses,
}

#[derive(Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: Url,
    api_key: String,
    provider: Provider,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        let base_url = Url::parse(base_url).context("base_url不是合法URL")?;
        let provider = detect_provider(&base_url);
        let http = reqwest::Client::builder().build()?;
        Ok(Self {
            http,
            base_url,
            api_key: api_key.to_string(),
            provider,
        })
    }

    pub async fn chat_completions(&self, req: ChatCompletionsRequest) -> Result<String> {
        match self.provider {
            Provider::OpenAiChatCompletions => self.chat_completions_openai(req).await,
            Provider::ArkResponses => self.chat_completions_ark_responses(req).await,
        }
    }

    pub fn supports_data_url_images(&self) -> bool {
        true
    }

    async fn chat_completions_openai(&self, req: ChatCompletionsRequest) -> Result<String> {
        let url = self
            .base_url
            .join("/v1/chat/completions")
            .context("拼接 /v1/chat/completions 失败")?;

        let res = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await
            .context("请求LLM失败")?;

        let status = res.status();
        let body = res.text().await.context("读取LLM响应失败")?;
        if !status.is_success() {
            anyhow::bail!("LLM响应错误: status={status} body={body}");
        }

        let parsed = serde_json::from_str::<ChatCompletionsResponse>(&body)
            .context("解析LLM响应JSON失败")?;
        let content = parsed
            .choices
            .first()
            .and_then(|c| c.message.content.clone());
        Ok(match content {
            Some(AssistantContent::Text(s)) => s,
            Some(AssistantContent::Parts(parts)) => parts
                .into_iter()
                .filter_map(|p| match p {
                    AssistantContentPart::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            None => String::new(),
        })
    }

    async fn chat_completions_ark_responses(&self, req: ChatCompletionsRequest) -> Result<String> {
        let url = ark_responses_url(&self.base_url)?;
        let ark_req = ArkResponsesRequest {
            model: req.model,
            input: req.messages.into_iter().map(ark_from_chat_message).collect(),
        };

        let res = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&ark_req)
            .send()
            .await
            .context("请求Ark Responses失败")?;

        let status = res.status();
        let body = res.text().await.context("读取Ark响应失败")?;
        if !status.is_success() {
            anyhow::bail!("Ark响应错误: status={status} body={body}");
        }

        extract_responses_text(&body)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionsResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub message: AssistantMessage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    pub content: Option<AssistantContent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AssistantContent {
    Text(String),
    Parts(Vec<AssistantContentPart>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize)]
struct ArkResponsesRequest {
    model: String,
    input: Vec<ArkInputMessage>,
}

#[derive(Debug, Clone, Serialize)]
struct ArkInputMessage {
    role: String,
    content: Vec<ArkInputContent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
enum ArkInputContent {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "input_image")]
    InputImage { image_url: String },
}

fn detect_provider(base_url: &Url) -> Provider {
    let path = base_url.path().to_ascii_lowercase();
    if path.ends_with("/responses") || path.contains("/api/v3/responses") {
        return Provider::ArkResponses;
    }
    let host = base_url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host.contains("volces.com") && path.contains("/api/v3") {
        return Provider::ArkResponses;
    }
    Provider::OpenAiChatCompletions
}

fn ark_responses_url(base_url: &Url) -> Result<Url> {
    let path = base_url.path().to_ascii_lowercase();
    if path.ends_with("/responses") {
        return Ok(base_url.clone());
    }
    base_url
        .join("/api/v3/responses")
        .context("拼接 /api/v3/responses 失败")
}

fn ark_from_chat_message(m: ChatMessage) -> ArkInputMessage {
    let role = m.role;
    let content = match m.content {
        MessageContent::Text(t) => vec![ArkInputContent::InputText { text: t }],
        MessageContent::Parts(parts) => parts
            .into_iter()
            .map(|p| match p {
                ContentPart::Text { text } => ArkInputContent::InputText { text },
                ContentPart::ImageUrl { image_url } => ArkInputContent::InputImage { image_url: image_url.url },
            })
            .collect(),
    };
    ArkInputMessage { role, content }
}

fn extract_responses_text(body: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(body).context("解析Ark响应JSON失败")?;

    if let Some(s) = v.get("output_text").and_then(|x| x.as_str()) {
        return Ok(s.to_string());
    }

    if let Some(arr) = v.get("output").and_then(|x| x.as_array()) {
        let mut out = String::new();
        for item in arr {
            if let Some(content) = item.get("content").and_then(|x| x.as_array()) {
                for c in content {
                    if let Some(text) = c.get("text").and_then(|x| x.as_str()) {
                        out.push_str(text);
                    } else if let Some(text) = c.get("output_text").and_then(|x| x.as_str()) {
                        out.push_str(text);
                    }
                }
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }

    if let Some(s) = v
        .get("choices")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|x| x.as_str())
    {
        return Ok(s.to_string());
    }

    Ok(String::new())
}
