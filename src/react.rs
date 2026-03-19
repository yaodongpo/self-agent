use anyhow::{Context, Result};
use base64::Engine;

use crate::llm::{ChatCompletionsRequest, ChatMessage, ContentPart, ImageUrl, MessageContent, OpenAiClient};
use crate::screenshot;
use crate::tools::{execute_tool, ToolCall, ToolContext};
use crate::upload::UploadClient;

#[derive(Debug, Clone)]
pub struct AgentAutoFeedbackConfig {
    pub enabled: bool,
    pub max_rounds: usize,
    pub criteria: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AgentHardVerifier {
    Dom {
        url: String,
        selector: Option<String>,
        js: Option<String>,
        timeout_ms: u64,
    },
    Template {
        template_path: String,
        threshold: Option<f32>,
        timeout_seconds: u64,
    },
}

pub struct Agent {
    client: OpenAiClient,
    model: String,
    messages: Vec<ChatMessage>,
    tool_ctx: ToolContext,
    max_steps: usize,
    auto_feedback: AgentAutoFeedbackConfig,
    uploader: Option<UploadClient>,
    hard_verifier: Option<AgentHardVerifier>,
}

impl Agent {
    pub fn new(
        client: OpenAiClient,
        model: String,
        system_prompt: String,
        tool_ctx: ToolContext,
        max_steps: usize,
    ) -> Self {
        let messages = vec![ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text(system_prompt),
        }];
        Self {
            client,
            model,
            messages,
            tool_ctx,
            max_steps,
            auto_feedback: AgentAutoFeedbackConfig {
                enabled: false,
                max_rounds: 3,
                criteria: None,
            },
            uploader: None,
            hard_verifier: None,
        }
    }

    pub fn set_auto_feedback(&mut self, cfg: AgentAutoFeedbackConfig) {
        self.auto_feedback = cfg;
    }

    pub fn set_uploader(&mut self, uploader: Option<UploadClient>) {
        self.uploader = uploader;
    }

    pub fn set_hard_verifier(&mut self, verifier: Option<AgentHardVerifier>) {
        self.hard_verifier = verifier;
    }

    pub fn push_user_text(&mut self, text: String) {
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text(text),
        });
    }

    pub fn push_user_text_with_image_data_url(&mut self, text: String, data_url: String) {
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Parts(vec![
                ContentPart::Text { text },
                ContentPart::ImageUrl {
                    image_url: ImageUrl { url: data_url },
                },
            ]),
        });
    }

    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    pub async fn run_turn(&mut self) -> Result<String> {
        let mut feedback_rounds_used = 0usize;
        let mut tool_errors = 0usize;
        if self.auto_feedback.enabled {
            let _ = self
                .inject_screenshot_feedback("INITIAL_SCREEN", None, &mut feedback_rounds_used)
                .await;
        }

        for _ in 0..self.max_steps {
            let req = ChatCompletionsRequest {
                model: self.model.clone(),
                messages: self.messages.clone(),
                temperature: Some(0.2),
                max_tokens: Some(1500),
            };
            let raw = self.client.chat_completions(req).await?;
            let trimmed = raw.trim();

            let call = serde_json::from_str::<ToolCall>(trimmed);
            match call {
                Ok(call) => {
                    if call.action == "final" {
                        let final_text = call
                            .input
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| call.input.to_string());
                        self.messages.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: MessageContent::Text(final_text.clone()),
                        });
                        return Ok(final_text);
                    }

                    let tool_out = match execute_tool(&call, &self.tool_ctx).await {
                        Ok(s) => s,
                        Err(e) => {
                            tool_errors += 1;
                            format!("TOOL_ERROR {}\n{}", call.action, e)
                        }
                    };

                    if tool_errors > 6 {
                        return Ok("工具连续报错次数过多，已停止。请检查环境/权限/依赖，或降低任务难度。".to_string());
                    }

                    if call.action == "capture_screen" {
                        let text = format!("TOOL_RESULT {}\n", call.action);
                        self.messages.push(ChatMessage {
                            role: "user".to_string(),
                            content: MessageContent::Parts(vec![
                                ContentPart::Text { text },
                                ContentPart::ImageUrl {
                                    image_url: ImageUrl { url: tool_out.clone() },
                                },
                            ]),
                        });
                    } else {
                        let tool_msg = format!("TOOL_RESULT {}\n{}", call.action, tool_out.trim_end());
                        self.messages.push(ChatMessage {
                            role: "user".to_string(),
                            content: MessageContent::Text(tool_msg),
                        });
                    }

                    if call.action.starts_with("ui_") {
                        self.inject_hard_check().await?;
                    }

                    if self.auto_feedback.enabled {
                        self.inject_screenshot_feedback(&call.action, Some(&tool_out), &mut feedback_rounds_used)
                            .await?;
                    }
                }
                Err(_) => {
                    self.messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: MessageContent::Text(raw.clone()),
                    });
                    return Ok(raw);
                }
            }
        }

        Ok("达到最大推理步数，未产出final。请缩小问题或提高 --max-steps。".to_string())
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    async fn inject_hard_check(&mut self) -> Result<()> {
        let Some(verifier) = self.hard_verifier.clone() else {
            return Ok(());
        };

        let call = match verifier {
            AgentHardVerifier::Dom {
                url,
                selector,
                js,
                timeout_ms,
            } => ToolCall {
                action: "eval_dom".to_string(),
                input: serde_json::json!({
                    "url": url,
                    "selector": selector,
                    "js": js,
                    "timeout_ms": timeout_ms,
                }),
            },
            AgentHardVerifier::Template {
                template_path,
                threshold,
                timeout_seconds,
            } => ToolCall {
                action: "match_template".to_string(),
                input: serde_json::json!({
                    "template_path": template_path,
                    "threshold": threshold,
                    "timeout_seconds": timeout_seconds,
                }),
            },
        };

        let raw = match execute_tool(&call, &self.tool_ctx).await {
            Ok(s) => s,
            Err(e) => format!("HARD_CHECK_ERROR {}\n{}", call.action, e),
        };

        let verdict = hard_check_verdict(call.action.as_str(), &raw);
        let msg = serde_json::json!({
            "type": "hard_check",
            "tool": call.action,
            "verdict": verdict,
            "raw": raw,
        });

        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text(format!("HARD_CHECK\n{msg}")),
        });

        Ok(())
    }

    async fn inject_screenshot_feedback(
        &mut self,
        phase: &str,
        tool_out: Option<&str>,
        feedback_rounds_used: &mut usize,
    ) -> Result<()> {
        if *feedback_rounds_used >= self.auto_feedback.max_rounds {
            return Ok(());
        }
        *feedback_rounds_used += 1;

        let png = tokio::task::spawn_blocking(screenshot::capture_primary_png)
            .await
            .context("截图任务失败")??;
        let image_url = self.png_to_image_url(png).await?;

        let mut text = String::new();
        text.push_str("FEEDBACK_SCREENSHOT\n");
        text.push_str("phase=");
        text.push_str(phase);
        text.push('\n');
        if let Some(out) = tool_out {
            text.push_str("last_tool_output:\n");
            text.push_str(out.trim_end());
            text.push('\n');
        }
        if let Some(c) = &self.auto_feedback.criteria {
            text.push_str("success_criteria:\n");
            text.push_str(c.trim_end());
            text.push('\n');
        }
        text.push_str(
            "请根据截图与上下文评估当前是否已达成目标。若未达成，继续输出下一次工具调用JSON；若已达成，输出 final。\n",
        );

        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Parts(vec![
                ContentPart::Text { text },
                ContentPart::ImageUrl {
                    image_url: ImageUrl { url: image_url },
                },
            ]),
        });

        Ok(())
    }

    async fn png_to_image_url(&self, png: Vec<u8>) -> Result<String> {
        if self.client.supports_data_url_images() {
            return Ok(format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(png)
            ));
        }

        let uploader = self.uploader.clone().context(
            "当前模型不支持data url图片。请先配置上传服务，再启用自动截图评估闭环。",
        )?;
        uploader.upload_png(png).await
    }
}

pub fn build_system_prompt(persona_markdown: Option<String>) -> String {
    let persona = persona_markdown.unwrap_or_else(|| {
        r#"# Persona
你是一个个人使用的高效助理。
你会在不泄露敏感信息的前提下，尽量用工具完成任务，并给出可复现的结果。
"#
        .to_string()
    });

    format!(
        r##"{persona}

# 运行协议
你将以ReAct方式工作，但只输出结构化的JSON对象。

当你需要调用工具时，输出：
{{"action":"TOOL_NAME","input":{{...}}}}

当你要结束并给用户最终回答时，输出：
{{"action":"final","input":"..."}}

# 可用工具
- run_python: {{"code":"...","args":["..."],"timeout_seconds":120}}
- pip_install: {{"packages":["opencv-python","numpy"],"upgrade":false,"pre":false,"no_deps":false,"index_url":null,"extra_index_url":null,"timeout_seconds":600}}
- read_file: {{"path":"relative/or/abs","max_bytes":32768}}
- write_file: {{"path":"relative/or/abs","content":"...","overwrite":false}}
- list_dir: {{"path":"relative/or/abs"}}
- sleep_ms: {{"ms":500}}
- ui_click: {{"x":100,"y":200,"button":"left","clicks":1}}
- ui_type: {{"text":"hello"}}
- ui_keypress: {{"key":"enter","modifiers":["ctrl","shift"]}}
- ui_scroll: {{"delta_y":-300,"delta_x":0}}
- capture_screen: {{}}
- ocr_screen: {{"lang":"eng","timeout_seconds":120}}
- eval_screen_text: {{"must_contain":["OK"],"must_not_contain":["Error"],"timeout_seconds":120}}
- match_template: {{"template_path":"templates/ok.png","threshold":0.85,"timeout_seconds":120}}
- eval_dom: {{"url":"https://example.com","selector":"#submit","js":"document.title","timeout_ms":30000}}

# 自动硬判定
系统可能在你执行 ui_* 工具后，自动追加一条以 HARD_CHECK 开头的消息，里面包含结构化评估结果。
优先依据 HARD_CHECK.verdict 来判断是否成功；成功就输出 final，失败就继续下一次工具调用JSON。

# 约束
- 不要在输出中包含除JSON以外的任何文本
- 不要输出API Key或任何密钥
- 尽量生成可直接运行的Python脚本
"##
    )
}

fn hard_check_verdict(tool: &str, raw: &str) -> serde_json::Value {
    let v = serde_json::from_str::<serde_json::Value>(raw.trim());
    match (tool, v) {
        ("eval_dom", Ok(v)) => {
            let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
            if !ok {
                return serde_json::json!({ "ok": false });
            }
            let exists = v.get("exists").and_then(|x| x.as_bool());
            match exists {
                Some(true) => serde_json::json!({ "ok": true, "exists": true }),
                Some(false) => serde_json::json!({ "ok": false, "exists": false }),
                None => serde_json::json!({ "ok": true }),
            }
        }
        ("match_template", Ok(v)) => {
            let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
            let hit = v.get("hit").and_then(|x| x.as_bool()).unwrap_or(false);
            if ok && hit {
                serde_json::json!({ "ok": true, "hit": true, "score": v.get("score") })
            } else {
                serde_json::json!({ "ok": false, "hit": hit, "score": v.get("score") })
            }
        }
        _ => serde_json::json!({ "ok": false }),
    }
}
