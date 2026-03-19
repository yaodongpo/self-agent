use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub llm: LlmConfig,
    pub python: PythonConfig,
    pub persona_markdown_path: Option<PathBuf>,
    pub workspace_root: PathBuf,
    pub memory: MemoryConfig,
    pub dingtalk: Option<DingTalkConfig>,
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfigFile {
    pub llm: Option<LlmConfigFile>,
    pub python: Option<PythonConfigFile>,
    pub persona_markdown_path: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub memory: Option<MemoryConfigFile>,
    pub dingtalk: Option<DingTalkConfigFile>,
    pub telegram: Option<TelegramConfigFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfigFile {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PythonConfigFile {
    pub venv_path: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PythonConfig {
    pub venv_path: PathBuf,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfigFile {
    pub jsonl_path: Option<PathBuf>,
    pub md_path: Option<PathBuf>,
    pub summarize_every_seconds: Option<u64>,
    pub keep_last_messages: Option<usize>,
    pub min_messages_to_summarize: Option<usize>,
    pub summarize_model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryConfig {
    pub jsonl_path: PathBuf,
    pub md_path: PathBuf,
    pub summarize_every_seconds: u64,
    pub keep_last_messages: usize,
    pub min_messages_to_summarize: usize,
    pub summarize_model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DingTalkConfigFile {
    pub enabled: Option<bool>,
    pub listen: Option<String>,
    pub path: Option<String>,
    pub webhook_url: Option<String>,
    pub webhook_secret: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    pub listen: String,
    pub path: String,
    pub webhook_url: String,
    pub webhook_secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfigFile {
    pub enabled: Option<bool>,
    pub token: Option<String>,
    pub allowed_chat_ids: Option<Vec<i64>>,
    pub poll_interval_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub token: String,
    pub allowed_chat_ids: Vec<i64>,
    pub poll_interval_ms: u64,
}

pub fn load_config_file(path: &Path) -> Result<Option<AgentConfigFile>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("读取配置文件失败: {}", path.display()))?;
    let cfg = toml::from_str::<AgentConfigFile>(&raw)
        .with_context(|| format!("解析配置文件失败(TOML): {}", path.display()))?;
    Ok(Some(cfg))
}

#[allow(clippy::too_many_arguments)]
pub fn merge_config(
    from_file: Option<AgentConfigFile>,
    cli_api_key: Option<String>,
    cli_base_url: Option<String>,
    cli_model: Option<String>,
    cli_python_venv: Option<PathBuf>,
    cli_persona: Option<PathBuf>,
    cli_workspace_root: Option<PathBuf>,
    cli_memory_jsonl: Option<PathBuf>,
    cli_memory_md: Option<PathBuf>,
    cli_summarize_every_seconds: Option<u64>,
    cli_keep_last_messages: Option<usize>,
    cli_min_messages_to_summarize: Option<usize>,
    cli_summarize_model: Option<String>,
    cli_dingtalk_enabled: Option<bool>,
    cli_dingtalk_listen: Option<String>,
    cli_dingtalk_path: Option<String>,
    cli_dingtalk_webhook_url: Option<String>,
    cli_dingtalk_webhook_secret: Option<String>,
    cli_telegram_enabled: Option<bool>,
    cli_telegram_token: Option<String>,
    cli_telegram_allowed_chat_ids: Option<Vec<i64>>,
    cli_telegram_poll_interval_ms: Option<u64>,
) -> Result<EffectiveConfig> {
    let file_llm = from_file.as_ref().and_then(|c| c.llm.as_ref());
    let file_python = from_file.as_ref().and_then(|c| c.python.as_ref());
    let file_memory = from_file.as_ref().and_then(|c| c.memory.as_ref());
    let file_dingtalk = from_file.as_ref().and_then(|c| c.dingtalk.as_ref());
    let file_telegram = from_file.as_ref().and_then(|c| c.telegram.as_ref());

    let api_key = cli_api_key
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .or_else(|| file_llm.and_then(|c| c.api_key.clone()))
        .context("缺少API Key: 用 --api-key 或 OPENAI_API_KEY 或配置文件 llm.api_key")?;

    let base_url = cli_base_url
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
        .or_else(|| file_llm.and_then(|c| c.base_url.clone()))
        .unwrap_or_else(|| "https://api.openai.com".to_string());

    let model = cli_model
        .or_else(|| std::env::var("OPENAI_MODEL").ok())
        .or_else(|| file_llm.and_then(|c| c.model.clone()))
        .unwrap_or_else(|| "gpt-4o-mini".to_string());

    let venv_path = cli_python_venv
        .or_else(|| file_python.and_then(|c| c.venv_path.clone()))
        .context("缺少Python虚拟环境路径: 用 --python-venv 或配置文件 python.venv_path")?;

    let timeout_seconds = file_python.and_then(|c| c.timeout_seconds).unwrap_or(120);

    let persona_markdown_path = cli_persona
        .or_else(|| from_file.as_ref().and_then(|c| c.persona_markdown_path.clone()))
        .or_else(|| {
            let p = PathBuf::from("persona.md");
            p.exists().then_some(p)
        });

    let workspace_root = cli_workspace_root
        .or_else(|| from_file.as_ref().and_then(|c| c.workspace_root.clone()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let memory_jsonl_path = cli_memory_jsonl
        .or_else(|| file_memory.and_then(|c| c.jsonl_path.clone()))
        .unwrap_or_else(|| workspace_root.join("memory.jsonl"));

    let memory_md_path = cli_memory_md
        .or_else(|| file_memory.and_then(|c| c.md_path.clone()))
        .unwrap_or_else(|| workspace_root.join("memory.md"));

    let summarize_every_seconds = cli_summarize_every_seconds
        .or_else(|| file_memory.and_then(|c| c.summarize_every_seconds))
        .unwrap_or(600);

    let keep_last_messages = cli_keep_last_messages
        .or_else(|| file_memory.and_then(|c| c.keep_last_messages))
        .unwrap_or(40);

    let min_messages_to_summarize = cli_min_messages_to_summarize
        .or_else(|| file_memory.and_then(|c| c.min_messages_to_summarize))
        .unwrap_or(20);

    let summarize_model = cli_summarize_model.or_else(|| file_memory.and_then(|c| c.summarize_model.clone()));

    let dingtalk_enabled = cli_dingtalk_enabled.or_else(|| file_dingtalk.and_then(|c| c.enabled)).unwrap_or(false);
    let dingtalk = if dingtalk_enabled {
        let webhook_url = cli_dingtalk_webhook_url
            .or_else(|| file_dingtalk.and_then(|c| c.webhook_url.clone()))
            .context("启用钉钉后需要 webhook_url: 用 --dingtalk-webhook-url 或配置文件 dingtalk.webhook_url")?;
        Some(DingTalkConfig {
            listen: cli_dingtalk_listen
                .or_else(|| file_dingtalk.and_then(|c| c.listen.clone()))
                .unwrap_or_else(|| "127.0.0.1:8088".to_string()),
            path: cli_dingtalk_path
                .or_else(|| file_dingtalk.and_then(|c| c.path.clone()))
                .unwrap_or_else(|| "/dingtalk".to_string()),
            webhook_url,
            webhook_secret: cli_dingtalk_webhook_secret
                .or_else(|| file_dingtalk.and_then(|c| c.webhook_secret.clone())),
        })
    } else {
        None
    };

    let telegram_enabled = cli_telegram_enabled.or_else(|| file_telegram.and_then(|c| c.enabled)).unwrap_or(false);
    let telegram = if telegram_enabled {
        let token = cli_telegram_token
            .or_else(|| std::env::var("TELEGRAM_BOT_TOKEN").ok())
            .or_else(|| file_telegram.and_then(|c| c.token.clone()))
            .context("启用Telegram后需要token: 用 --telegram-token 或 TELEGRAM_BOT_TOKEN 或配置文件 telegram.token")?;
        let allowed_chat_ids = cli_telegram_allowed_chat_ids
            .or_else(|| file_telegram.and_then(|c| c.allowed_chat_ids.clone()))
            .unwrap_or_default();
        let poll_interval_ms = cli_telegram_poll_interval_ms
            .or_else(|| file_telegram.and_then(|c| c.poll_interval_ms))
            .unwrap_or(1200);
        Some(TelegramConfig {
            token,
            allowed_chat_ids,
            poll_interval_ms,
        })
    } else {
        None
    };

    Ok(EffectiveConfig {
        llm: LlmConfig {
            api_key,
            base_url,
            model,
        },
        python: PythonConfig {
            venv_path,
            timeout_seconds,
        },
        persona_markdown_path,
        workspace_root,
        memory: MemoryConfig {
            jsonl_path: memory_jsonl_path,
            md_path: memory_md_path,
            summarize_every_seconds,
            keep_last_messages,
            min_messages_to_summarize,
            summarize_model,
        },
        dingtalk,
        telegram,
    })
}
