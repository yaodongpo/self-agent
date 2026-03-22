use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Url;
use serde::Deserialize;

#[derive(Clone)]
pub struct UploadClient {
    http: reqwest::Client,
    endpoint: Url,
    api_key: Option<String>,
}

impl UploadClient {
    pub fn new(endpoint: &str, api_key: Option<String>) -> Result<Self> {
        let endpoint = Url::parse(endpoint).context("upload endpoint 不是合法URL")?;
        Ok(Self {
            http: reqwest::Client::new(),
            endpoint,
            api_key,
        })
    }

    pub async fn upload_png(&self, bytes: Vec<u8>) -> Result<String> {
        self.upload_bytes("image/png", &bytes).await
    }

    pub async fn upload_bytes(&self, mime: &str, bytes: &[u8]) -> Result<String> {
        let body = serde_json::json!({
            "mime": mime,
            "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        });

        let mut req = self.http.post(self.endpoint.clone()).json(&body);
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }
        let res = req.send().await.context("上传请求失败")?;
        let status = res.status();
        let text = res.text().await.context("读取上传响应失败")?;
        if !status.is_success() {
            anyhow::bail!("上传响应错误: status={status} body={text}");
        }
        let parsed = serde_json::from_str::<UploadResponse>(&text).context("解析上传响应JSON失败")?;
        let url = parsed
            .url
            .or_else(|| parsed.data.and_then(|d| d.url))
            .context("上传响应缺少url字段")?;
        Ok(url)
    }
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    data: Option<UploadData>,
}

#[derive(Debug, Deserialize)]
struct UploadData {
    #[serde(default)]
    url: Option<String>,
}

