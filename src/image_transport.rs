use anyhow::{Context, Result};
use base64::Engine;

use crate::upload::UploadClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageTransportMode {
    Inline,
    Upload,
    Auto,
}

impl ImageTransportMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "inline" | "dataurl" | "data_url" => Some(Self::Inline),
            "upload" | "url" => Some(Self::Upload),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

pub async fn image_url_from_bytes(
    mime: &str,
    bytes: &[u8],
    mode: ImageTransportMode,
    uploader: Option<UploadClient>,
) -> Result<String> {
    match mode {
        ImageTransportMode::Inline => Ok(to_data_url(mime, bytes)),
        ImageTransportMode::Upload => {
            let u = uploader.context("未配置上传服务(uploader)，无法返回图片URL")?;
            u.upload_bytes(mime, bytes).await
        }
        ImageTransportMode::Auto => {
            if let Some(u) = uploader {
                let approx_data_url_len = (bytes.len() * 4 / 3) + 64;
                if approx_data_url_len > 1_500_000 {
                    return u.upload_bytes(mime, bytes).await;
                }
            }
            Ok(to_data_url(mime, bytes))
        }
    }
}

fn to_data_url(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

