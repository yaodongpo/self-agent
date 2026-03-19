use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::llm::{ChatCompletionsRequest, ChatMessage, MessageContent, OpenAiClient};

#[derive(Debug, Clone)]
pub struct MemoryConfig {
    pub jsonl_path: PathBuf,
    pub md_path: PathBuf,
    pub summarize_every_seconds: u64,
    pub keep_last_messages: usize,
    pub min_messages_to_summarize: usize,
    pub summarize_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryLine {
    role: String,
    content: String,
}

pub struct MemoryManager {
    cfg: MemoryConfig,
    last_compacted_len: usize,
}

impl MemoryManager {
    pub fn new(cfg: MemoryConfig) -> Result<Self> {
        ensure_md_exists(&cfg.md_path)?;
        let lines = load_jsonl_lines(&cfg.jsonl_path).unwrap_or_default();
        Ok(Self {
            cfg,
            last_compacted_len: lines.len(),
        })
    }

    pub fn load_messages(&self) -> Result<Vec<ChatMessage>> {
        let lines = load_jsonl_lines(&self.cfg.jsonl_path)?;
        Ok(lines
            .into_iter()
            .filter(|m| m.role != "system")
            .map(|m| ChatMessage {
                role: m.role,
                content: MessageContent::Text(m.content),
            })
            .collect())
    }

    pub fn append_messages(&mut self, messages: &[ChatMessage]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.cfg.jsonl_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut buf = String::new();
        for m in messages {
            if m.role == "system" {
                continue;
            }
            let content = content_to_text(&m.content);
            let line = MemoryLine {
                role: m.role.clone(),
                content,
            };
            buf.push_str(&serde_json::to_string(&line)?);
            buf.push('\n');
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.cfg.jsonl_path)
            .with_context(|| format!("打开记忆JSONL失败: {}", self.cfg.jsonl_path.display()))?;
        f.write_all(buf.as_bytes()).context("写入记忆JSONL失败")?;
        Ok(())
    }

    pub async fn maybe_compact(&mut self, client: &OpenAiClient, default_model: &str) -> Result<bool> {
        let lines = load_jsonl_lines(&self.cfg.jsonl_path).unwrap_or_default();
        if lines.len().saturating_sub(self.last_compacted_len) < self.cfg.min_messages_to_summarize {
            return Ok(false);
        }
        if lines.len() <= self.cfg.keep_last_messages + self.cfg.min_messages_to_summarize {
            self.last_compacted_len = lines.len();
            return Ok(false);
        }

        let split_at = lines.len().saturating_sub(self.cfg.keep_last_messages);
        let (older, keep) = lines.split_at(split_at);
        let older_text = older
            .iter()
            .map(|l| format!("{}: {}", l.role, l.content))
            .collect::<Vec<_>>()
            .join("\n");

        let current_md = std::fs::read_to_string(&self.cfg.md_path).unwrap_or_default();
        let model = self
            .cfg
            .summarize_model
            .clone()
            .unwrap_or_else(|| default_model.to_string());

        let prompt = build_summarize_prompt(&current_md, &older_text);
        let req = ChatCompletionsRequest {
            model,
            messages: vec![ChatMessage {
                role: "system".to_string(),
                content: MessageContent::Text(
                    "你是记忆压缩器。你只输出Markdown，不要输出代码块围栏。".to_string(),
                ),
            }, ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text(prompt),
            }],
            temperature: Some(0.2),
            max_tokens: Some(1200),
        };
        let summary = client.chat_completions(req).await?;

        let new_md = build_memory_md(&summary, keep);
        std::fs::write(&self.cfg.md_path, new_md.as_bytes())
            .with_context(|| format!("写入memory.md失败: {}", self.cfg.md_path.display()))?;

        rewrite_jsonl(&self.cfg.jsonl_path, keep)?;
        self.last_compacted_len = keep.len();
        Ok(true)
    }
}

fn ensure_md_exists(path: &PathBuf) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let initial = "# Memory\n\n## Summary\n\n\n## Recent\n\n";
    std::fs::write(path, initial.as_bytes()).with_context(|| format!("创建memory.md失败: {}", path.display()))?;
    Ok(())
}

fn load_jsonl_lines(path: &PathBuf) -> Result<Vec<MemoryLine>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("读取记忆JSONL失败: {}", path.display()))?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let m: MemoryLine = serde_json::from_str(line).context("解析记忆JSONL失败")?;
        out.push(m);
    }
    Ok(out)
}

fn rewrite_jsonl(path: &PathBuf, keep: &[MemoryLine]) -> Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut buf = String::new();
    for l in keep {
        buf.push_str(&serde_json::to_string(l)?);
        buf.push('\n');
    }
    std::fs::write(path, buf.as_bytes()).with_context(|| format!("重写记忆JSONL失败: {}", path.display()))?;
    Ok(())
}

fn build_summarize_prompt(current_md: &str, older_text: &str) -> String {
    format!(
        r#"你将维护一个长期记忆文件 memory.md。

目标：把“新增对话内容”压缩成稳定、可用的记忆摘要，便于未来继续对话。

输出格式必须是Markdown，包含以下小节，顺序固定：

## Summary
- 用要点列出长期不变或重要的信息（偏好、常用路径/环境、项目背景）

## Tasks
- 列出未完成任务或待办（尽量短）

## Facts
- 重要事实、账号系统名称、服务地址等（不要包含密钥）

## Notes
- 其它补充

已有 memory.md 内容（可能为空）：
{current_md}

新增对话内容（需要压缩）：
{older_text}
"#
    )
}

fn build_memory_md(summary_md: &str, keep: &[MemoryLine]) -> String {
    let mut out = String::new();
    out.push_str("# Memory\n\n");
    out.push_str(summary_md.trim());
    out.push_str("\n\n## Recent\n");
    for l in keep {
        let content = l.content.replace('\n', " ");
        out.push_str("- ");
        out.push_str(&l.role);
        out.push_str(": ");
        out.push_str(&content);
        out.push('\n');
    }
    out
}

fn content_to_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => {
            let mut out = String::new();
            for p in parts {
                match p {
                    crate::llm::ContentPart::Text { text } => out.push_str(text),
                    crate::llm::ContentPart::ImageUrl { .. } => out.push_str("\n[image]\n"),
                }
            }
            out
        }
    }
}
