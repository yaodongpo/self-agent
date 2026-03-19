# self-agent

一个个人使用的 Rust Agent：支持 ReAct、多模态（截图/图片）、Python 脚本技能执行、对话记忆压缩（memory.md），并可对接钉钉/Telegram 用消息控制。当前已适配 OpenAI Chat Completions 与豆包/火山 Ark Responses 两类接口（按 base_url 自动路由）。

## 快速开始

### 1) 配置 Python 虚拟环境

Agent 的 OCR / DOM 评估 / 模板匹配等“结构化评估器”依赖 Python 环境执行。请先指定你要用的虚拟环境目录（或 python.exe 路径）。

修改 `agent.toml`：

```toml
[python]
venv_path = "E:/path/to/your/venv"
timeout_seconds = 120
```

Windows 常见路径：
- venv 目录：`D:/.../envs/xxx/`（程序会尝试寻找 python 可执行文件）
- 你也可以直接把 `venv_path` 指向 `python.exe`

### 2) 配置大模型

支持两种接口形态：
- OpenAI Chat Completions：`POST /v1/chat/completions`
- 豆包/火山 Ark Responses：`POST /api/v3/responses`

二选一：

- 环境变量：`OPENAI_API_KEY=...`（可选 `OPENAI_BASE_URL` / `OPENAI_MODEL`）
- 或 `agent.toml`：

```toml
[llm]
base_url = "https://api.openai.com"
model = "gpt-4o-mini"
```

豆包/火山（Ark Responses）示例：

```toml
[llm]
base_url = "https://ark.cn-beijing.volces.com/api/v3/responses"
model = "doubao-seed-2-0-pro-260215"
```

### 3) 运行

交互式（REPL）：

```bash
cargo run -- --config agent.toml
```

单次模式：

```bash
cargo run -- --config agent.toml --once "帮我写个脚本统计当前目录最大的10个文件"
```

附带截图（每条消息都截一张附上）：

```bash
cargo run -- --config agent.toml --screenshot
```

说明：
- 截图会在本地截取后自动将最长边缩放到 720px，再编码为 PNG
- 图片以 `data:image/png;base64,...` 形式内联到请求里（image_url）

发送指定图片文件（不截屏）：

```bash
cargo run -- --config agent.toml --image E:\tmp\screen.png --once "这张图里发生了什么？"
```

## 命令行参数速查

常用参数：
- `--config agent.toml`：指定配置文件
- `--once "..."`：单次运行
- `--max-steps 8`：单次 turn 内最大工具步数
- `--screenshot`：每次用户输入自动附带截图
- `--image path.png`：附带指定图片（优先于 screenshot）
- `--no-memory`：禁用记忆读写与压缩

自动反馈闭环：
- `--auto-feedback`：开启“自动截图→评估→继续执行”
- `--auto-feedback-max-rounds 3`：自动截图评估最多轮数
- `--auto-feedback-criteria "..."`：成功标准（越明确越稳）

上传服务：
- `--upload-endpoint http://.../upload` 或 `SELF_AGENT_UPLOAD_ENDPOINT`（可选）
- `--upload-api-key TOKEN` 或 `SELF_AGENT_UPLOAD_API_KEY`（可选）
说明：当前实现默认走 data-url 直传图片，不再依赖上传服务；上传服务更多用于你后续切回“URL 传图”的模式或做外部集成。

UI 动作后硬判定（二选一）：
- DOM：
  - `--hard-dom-url URL`
  - `--hard-dom-selector CSS`（可选）
  - `--hard-dom-js "..."`（可选，page.evaluate）
  - `--hard-dom-timeout-ms 30000`
- 模板：
  - `--hard-template-path templates/ok.png`
  - `--hard-template-threshold 0.85`（可选）
  - `--hard-template-timeout-seconds 120`

## 工具与能力概览

Agent 以结构化 JSON 调工具，核心工具包括：
- 文件：read_file / write_file / list_dir
- Python：run_python（在指定 venv 内执行模型生成脚本）、pip_install（给 venv 装依赖）
- UI 自动化：ui_click / ui_type / ui_keypress / ui_scroll / sleep_ms
- 视觉：capture_screen（返回图片 data url；截图会压缩到最长边 720px）
- 结构化评估器：
  - OCR：ocr_screen / eval_screen_text
  - 模板匹配：match_template（OpenCV）
  - DOM 评估：eval_dom（Python Playwright）

## 自动截图→评估→继续执行（闭环）

开启后，Agent 会在关键阶段自动截图，把“工具输出 + 截图 + 成功标准”发给模型做评估；若未达成目标，模型继续输出下一步工具调用 JSON，直至 final 或达到步数上限。

开启：

```bash
cargo run -- --config agent.toml --auto-feedback
```

配置最大反馈轮数与成功标准：

```bash
cargo run -- --config agent.toml --auto-feedback --auto-feedback-max-rounds 8 --auto-feedback-criteria "出现Success字样且无Error弹窗"
```

### 可选：截图上传服务（当你需要 URL 传图时）

当前实现默认用 data-url（base64）直接传图，不再经过上传服务换 URL。

如果你后续遇到某些模型/网关不接受 data-url，才需要切换为“先上传拿 URL，再传 image_url”的模式（届时会用到 upload-service / S3 / 反向代理等）。

启动参数（或环境变量）：

```bash
cargo run -- --config agent.toml --auto-feedback --upload-endpoint http://127.0.0.1:9000/upload
```

环境变量：
- SELF_AGENT_UPLOAD_ENDPOINT
- SELF_AGENT_UPLOAD_API_KEY（可选，Bearer）

上传协议约定：
- 请求：POST JSON `{"mime":"image/png","data_base64":"..."}`
- 响应：JSON `{"url":"https://..."}` 或 `{"data":{"url":"https://..."}}`

### 本仓库自带的本地 upload-service

仓库内提供了一个最小可用的上传服务： [cmd/upload-service](file:///e:/code/rust/self-agent/cmd/upload-service)

启动：

```bash
cd cmd/upload-service
python server.py
```

带 token：

```bash
python server.py --token YOUR_TOKEN
```

然后 self-agent 侧配置：

```bash
cargo run -- --config agent.toml --upload-endpoint http://127.0.0.1:9000/upload --upload-api-key YOUR_TOKEN
```

重要说明：
- 这个服务默认返回 `http://127.0.0.1:9000/files/...`。如果你的模型运行在云端（例如豆包/Ark），它通常无法访问你本机的 `127.0.0.1`。
- 真实接入云端多模态时，你需要让 `url` 对模型可访问：例如部署到公网、配反向代理/内网穿透（ngrok、frp、cloudflared），或换成对象存储（TOS/S3）签名 URL。

## UI 动作后的硬判定（优先使用 DOM/模板匹配）

你可以配置“硬判定规则”，让 Agent 在执行完 `ui_*` 工具后自动跑一次结构化评估（eval_dom / match_template），并把评估结果以 `HARD_CHECK` 消息注入上下文。模型应优先根据 `HARD_CHECK.verdict.ok` 决定是否 final。

### 方式 A：DOM 硬判定（网页更稳）

```bash
cargo run -- --config agent.toml ^
  --hard-dom-url "https://example.com" ^
  --hard-dom-selector "#submit" ^
  --hard-dom-timeout-ms 30000
```

规则：
- eval_dom 返回 `ok=true`
- 若返回 `exists` 字段，则要求 `exists=true`

### 方式 B：模板匹配硬判定（桌面/固定 UI）

```bash
cargo run -- --config agent.toml ^
  --hard-template-path "templates/ok.png" ^
  --hard-template-threshold 0.85
```

规则：
- match_template 返回 `ok=true` 且 `hit=true`

## 记忆（memory.md）

默认记录增量到 `memory.jsonl`，并定期压缩到 `memory.md`。

示例配置：

```toml
[memory]
jsonl_path = "memory.jsonl"
md_path = "memory.md"
summarize_every_seconds = 600
keep_last_messages = 40
min_messages_to_summarize = 20
```

禁用记忆：

```bash
cargo run -- --config agent.toml --no-memory
```

## 结构化评估器依赖（Python venv）

### 自动安装依赖（pip_install）

Agent 现在支持通过工具 `pip_install` 给当前 venv 自动安装依赖（等价于 `python -m pip install ...`）。

典型用法（让模型先装依赖再调用评估器）：

- 安装 OpenCV 模板匹配依赖：
  - `opencv-python` / `numpy`
- 安装 Playwright（DOM 评估）依赖：
  - `playwright`（并且通常还需要执行 `playwright install` 下载浏览器）

注意：
- pip_install 会在你配置的 `python.venv_path` 指向的解释器里执行
- 该操作会改动你的 venv，请在可控环境使用

pip_install 工具入参示例（JSON）：

```json
{"action":"pip_install","input":{"packages":["opencv-python","numpy"],"upgrade":false,"pre":false,"no_deps":false,"index_url":null,"extra_index_url":null,"timeout_seconds":600}}
```

### OCR
二选一：
- `pip install pillow pytesseract`（并确保系统已安装 tesseract OCR）
- 或 `pip install easyocr`

### 模板匹配（OpenCV）
- `pip install opencv-python numpy`

### DOM 评估（Playwright）
- `pip install playwright`
- `playwright install`

## 钉钉对接

在 `agent.toml` 启用：

```toml
[dingtalk]
enabled = true
listen = "127.0.0.1:8088"
path = "/dingtalk"
webhook_url = "https://oapi.dingtalk.com/robot/send?access_token=REPLACE_ME"
```

说明：
- 回调地址为 `http://127.0.0.1:8088/dingtalk`
- 钉钉发来的文本会被转发给 Agent，Agent 输出会通过 webhook 发回群里

## Telegram 对接

在 `agent.toml` 启用：

```toml
[telegram]
enabled = true
token = "123456:bot_token_here"
allowed_chat_ids = [123456789]
poll_interval_ms = 1200
```

说明：
- 使用 long polling 获取更新（getUpdates）
- 仅处理 `allowed_chat_ids` 内的消息（避免被陌生人控制）
- 收到文本后转发给 Agent，并用 sendMessage 回复

## 排障

### 1) Ark 模型看不到图片 / image_url 无效
- 确保 `--upload-endpoint` 返回的是“模型可访问”的公网 URL（不要是 `127.0.0.1`）
- 如果你只是本机自测（模型也在本机/局域网可访问），再用本地 URL

### 2) OCR / 模板匹配 / eval_dom 报依赖缺失
- 先用 pip_install 安装依赖：
  - OpenCV：`opencv-python numpy`
  - Playwright：`playwright` + 执行 `playwright install`
  - OCR：`pillow pytesseract`（并安装 tesseract）或 `easyocr`

### 3) UI 自动化无效/点错
- UI 工具基于系统输入注入，受焦点窗口、权限、缩放/DPI 影响很大
- 建议配合 `capture_screen` / `ocr_screen` / `match_template` 做闭环确认

### 4) base_url 与接口不匹配
- OpenAI：`https://api.openai.com`
- Ark Responses：`https://ark.../api/v3/responses`（当前实现会按 base_url 自动路由）

## 安全与注意事项

- UI 自动化（enigo）会对真实系统发送鼠标键盘事件：建议在可控环境中使用，避免误操作。
- `run_python` 会执行模型生成的脚本：建议默认在隔离的工作目录使用，避免脚本误删/泄露本机数据。
- 不要把密钥写进公开仓库。更推荐使用环境变量注入。
