mod config;
mod dingtalk;
mod image_transport;
mod llm;
mod memory;
mod react;
mod screenshot;
mod telegram;
mod tools;
mod upload;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use directories::ProjectDirs;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use tokio::sync::Mutex;

use crate::config::{load_config_file, merge_config};
use crate::image_transport::{image_url_from_bytes, ImageTransportMode};
use crate::llm::OpenAiClient;
use crate::memory::{MemoryConfig, MemoryManager};
use crate::react::{build_system_prompt, Agent, AgentAutoFeedbackConfig, AgentHardVerifier, AgentTraceConfig};
use crate::tools::ToolContext;
use crate::upload::UploadClient;

#[derive(Debug, Parser)]
#[command(name = "self-agent")]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,

    #[arg(long, env = "OPENAI_API_KEY")]
    api_key: Option<String>,

    #[arg(long, env = "OPENAI_BASE_URL")]
    base_url: Option<String>,

    #[arg(long, env = "OPENAI_MODEL")]
    model: Option<String>,

    #[arg(long)]
    persona: Option<PathBuf>,

    #[arg(long)]
    workspace_root: Option<PathBuf>,

    #[arg(long, env = "SELF_AGENT_PYTHON_VENV")]
    python_venv: Option<PathBuf>,

    #[arg(long, default_value_t = 8)]
    max_steps: usize,

    #[arg(long)]
    screenshot: bool,

    #[arg(long)]
    image: Option<PathBuf>,

    #[arg(long, env = "SELF_AGENT_IMAGE_TRANSPORT", default_value = "inline")]
    image_transport: String,

    #[arg(long)]
    once: Option<String>,

    #[arg(long)]
    memory_jsonl: Option<PathBuf>,

    #[arg(long)]
    memory_md: Option<PathBuf>,

    #[arg(long)]
    summarize_every_seconds: Option<u64>,

    #[arg(long)]
    keep_last_messages: Option<usize>,

    #[arg(long)]
    min_messages_to_summarize: Option<usize>,

    #[arg(long)]
    summarize_model: Option<String>,

    #[arg(long, default_value_t = false)]
    no_memory: bool,

    #[arg(long)]
    dingtalk_enabled: Option<bool>,

    #[arg(long)]
    dingtalk_listen: Option<String>,

    #[arg(long)]
    dingtalk_path: Option<String>,

    #[arg(long)]
    dingtalk_webhook_url: Option<String>,

    #[arg(long)]
    dingtalk_webhook_secret: Option<String>,

    #[arg(long)]
    telegram_enabled: Option<bool>,

    #[arg(long, env = "TELEGRAM_BOT_TOKEN")]
    telegram_token: Option<String>,

    #[arg(long)]
    telegram_allowed_chat_ids: Option<Vec<i64>>,

    #[arg(long)]
    telegram_poll_interval_ms: Option<u64>,

    #[arg(long, default_value_t = false)]
    auto_feedback: bool,

    #[arg(long, default_value_t = 3)]
    auto_feedback_max_rounds: usize,

    #[arg(long)]
    auto_feedback_criteria: Option<String>,

    #[arg(long, env = "SELF_AGENT_UPLOAD_ENDPOINT")]
    upload_endpoint: Option<String>,

    #[arg(long, env = "SELF_AGENT_UPLOAD_API_KEY")]
    upload_api_key: Option<String>,

    #[arg(long)]
    hard_dom_url: Option<String>,

    #[arg(long)]
    hard_dom_selector: Option<String>,

    #[arg(long)]
    hard_dom_js: Option<String>,

    #[arg(long, default_value_t = 30_000)]
    hard_dom_timeout_ms: u64,

    #[arg(long)]
    hard_template_path: Option<String>,

    #[arg(long)]
    hard_template_threshold: Option<f32>,

    #[arg(long, default_value_t = 120)]
    hard_template_timeout_seconds: u64,

    #[arg(long, default_value_t = true)]
    trace: bool,

    #[arg(long, default_value_t = true)]
    trace_llm_raw_on_parse_error: bool,

    #[arg(long, default_value_t = 400)]
    trace_max_preview_chars: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg_path = cli.config.clone().unwrap_or_else(default_config_path);
    let from_file = load_config_file(&cfg_path)?;
    let cfg = merge_config(
        from_file,
        cli.api_key,
        cli.base_url,
        cli.model,
        cli.python_venv,
        cli.persona.clone(),
        cli.workspace_root,
        cli.memory_jsonl.clone(),
        cli.memory_md.clone(),
        cli.summarize_every_seconds,
        cli.keep_last_messages,
        cli.min_messages_to_summarize,
        cli.summarize_model.clone(),
        cli.dingtalk_enabled,
        cli.dingtalk_listen.clone(),
        cli.dingtalk_path.clone(),
        cli.dingtalk_webhook_url.clone(),
        cli.dingtalk_webhook_secret.clone(),
        cli.telegram_enabled,
        cli.telegram_token.clone(),
        cli.telegram_allowed_chat_ids.clone(),
        cli.telegram_poll_interval_ms,
    )?;

    let persona_markdown = if let Some(p) = &cfg.persona_markdown_path {
        std::fs::read_to_string(p)
            .with_context(|| format!("读取persona markdown失败: {}", p.display()))
            .ok()
    } else {
        None
    };

    let system_prompt = build_system_prompt(persona_markdown);
    let client = OpenAiClient::new(&cfg.llm.base_url, &cfg.llm.api_key)?;

    let uploader = match cli.upload_endpoint.as_deref() {
        Some(ep) if !ep.trim().is_empty() => Some(UploadClient::new(ep, cli.upload_api_key.clone())?),
        _ => None,
    };
    let uploader_cloned = uploader.clone();
    let uploader_cloned1 = uploader.clone();
    let uploader_cloned2 = uploader.clone();

    let image_transport = ImageTransportMode::parse(&cli.image_transport)
        .context("image_transport 仅支持: inline|upload|auto")?;

    let tool_ctx = ToolContext {
        python_venv_path: cfg.python.venv_path.clone(),
        python_timeout_seconds: cfg.python.timeout_seconds,
        workspace_root: cfg.workspace_root.clone(),
        uploader: uploader_cloned,
        image_transport,
    };
    let mut agent = Agent::new(
        client.clone(),
        cfg.llm.model.clone(),
        system_prompt,
        tool_ctx,
        cli.max_steps,
    );

    if cli.auto_feedback {
        let cfg = AgentAutoFeedbackConfig {
            enabled: true,
            max_rounds: cli.auto_feedback_max_rounds,
            criteria: cli.auto_feedback_criteria.clone(),
        };
        agent.set_auto_feedback(cfg);
    }

    agent.set_uploader(uploader);

    let hard = if let Some(url) = cli.hard_dom_url.clone().filter(|s| !s.trim().is_empty()) {
        Some(AgentHardVerifier::Dom {
            url,
            selector: cli.hard_dom_selector.clone().filter(|s| !s.trim().is_empty()),
            js: cli.hard_dom_js.clone().filter(|s| !s.trim().is_empty()),
            timeout_ms: cli.hard_dom_timeout_ms,
        })
    } else if let Some(p) = cli.hard_template_path.clone().filter(|s| !s.trim().is_empty()) {
        Some(AgentHardVerifier::Template {
            template_path: p,
            threshold: cli.hard_template_threshold,
            timeout_seconds: cli.hard_template_timeout_seconds,
        })
    } else {
        None
    };
    agent.set_hard_verifier(hard);

    agent.set_trace(AgentTraceConfig {
        enabled: cli.trace,
        show_llm_raw_on_parse_error: cli.trace_llm_raw_on_parse_error,
        max_preview_chars: cli.trace_max_preview_chars,
    });

    let memory_cfg = MemoryConfig {
        jsonl_path: cfg.memory.jsonl_path.clone(),
        md_path: cfg.memory.md_path.clone(),
        summarize_every_seconds: cfg.memory.summarize_every_seconds,
        keep_last_messages: cfg.memory.keep_last_messages,
        min_messages_to_summarize: cfg.memory.min_messages_to_summarize,
        summarize_model: cfg.memory.summarize_model.clone(),
    };
    let every = memory_cfg.summarize_every_seconds;
    let memory = Arc::new(Mutex::new(MemoryManager::new(memory_cfg)?));

    if !cli.no_memory {
        let msgs = memory.lock().await.load_messages().unwrap_or_default();
        for m in msgs {
            agent.push_message(m);
        }
    }

    let agent = Arc::new(Mutex::new(agent));
    let saved_len = Arc::new(Mutex::new(agent.lock().await.messages().len()));

    if !cli.no_memory && every > 0 {
        let mem = memory.clone();
        let client2 = client.clone();
        let default_model = cfg.llm.model.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(every));
            loop {
                interval.tick().await;
                let mut m = mem.lock().await;
                let _ = m.maybe_compact(&client2, &default_model).await;
            }
        });
    }

    if let Some(dt) = cfg.dingtalk.clone() {
        let listen: std::net::SocketAddr = dt.listen.parse().context("dingtalk.listen 不是合法地址")?;
        let cfg_dt = crate::dingtalk::DingTalkConfig {
            listen,
            path: dt.path,
            webhook_url: dt.webhook_url,
            webhook_secret: dt.webhook_secret,
        };
        let agent2 = agent.clone();
        let mem2 = memory.clone();
        tokio::spawn(async move {
            let _ = crate::dingtalk::run_server(cfg_dt, agent2, mem2).await;
        });
    }

    if let Some(tg) = cfg.telegram.clone() {
        let agent2 = agent.clone();
        let mem2 = memory.clone();
        tokio::spawn(async move {
            let _ = crate::telegram::run_bot(tg, agent2, mem2).await;
        });
    }
    if let Some(text) = cli.once {
                {
                    let mut a = agent.lock().await;
                    push_user_input(
                        &mut a,
                        text,
                        cli.screenshot,
                        cli.image,
                        uploader_cloned1,
                        image_transport,
                    )
                    .await?;
                }
        let out = {
            let mut a = agent.lock().await;
            a.run_turn().await?
        };
        if !cli.no_memory {
            persist_new_messages(&agent, &memory, &saved_len).await?;
        }
        println!("{out}");
        return Ok(());
    }

    let mut rl = DefaultEditor::new()?;
    loop {
        let line = rl.readline("you> ");
        match line {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if line == "/exit" || line == "/quit" {
                    break;
                }
                rl.add_history_entry(line.as_str())?;
                {
                    let mut a = agent.lock().await;
                    push_user_input(
                        &mut a,
                        line,
                        cli.screenshot,
                        cli.image.clone(),
                        uploader_cloned2.clone(),
                        image_transport,
                    )
                    .await?;
                }
                let out = {
                    let mut a = agent.lock().await;
                    a.run_turn().await?
                };
                if !cli.no_memory {
                    persist_new_messages(&agent, &memory, &saved_len).await?;
                }
                println!("agent> {out}");
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

async fn push_user_input(
    agent: &mut Agent,
    text: String,
    with_screenshot: bool,
    image_path: Option<PathBuf>,
    uploader: Option<UploadClient>,
    image_transport: ImageTransportMode,
) -> Result<()> {
    if let Some(p) = image_path {
        let (mime, bytes) = read_image_bytes(&p)?;
        let url = image_url_from_bytes(&mime, &bytes, image_transport, uploader).await?;
        agent.push_user_text_with_image_data_url(text, url);
        return Ok(());
    }

    if with_screenshot {
        let png = screenshot::capture_primary_png()?;
        let url = image_url_from_bytes("image/png", &png, image_transport, uploader).await?;
        agent.push_user_text_with_image_data_url(text, url);
        return Ok(());
    }

    agent.push_user_text(text);
    Ok(())
}

fn read_image_bytes(path: &PathBuf) -> Result<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).with_context(|| format!("读取图片失败: {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok((mime.to_string(), bytes))
}

fn default_config_path() -> PathBuf {
    let local = PathBuf::from("agent.toml");
    if local.exists() {
        return local;
    }
    if let Some(proj) = ProjectDirs::from("", "", "self-agent") {
        let dir = proj.config_dir();
        let _ = std::fs::create_dir_all(dir);
        dir.join("agent.toml")
    } else {
        PathBuf::from("agent.toml")
    }
}

async fn persist_new_messages(
    agent: &Arc<Mutex<Agent>>,
    memory: &Arc<Mutex<MemoryManager>>,
    saved_len: &Arc<Mutex<usize>>,
) -> Result<()> {
    let new_msgs = {
        let mut saved = saved_len.lock().await;
        let a = agent.lock().await;
        let slice = &a.messages()[*saved..];
        let out = slice.to_vec();
        *saved += out.len();
        out
    };
    if !new_msgs.is_empty() {
        let mut mem = memory.lock().await;
        mem.append_messages(&new_msgs)?;
    }
    Ok(())
}
