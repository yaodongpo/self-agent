# self-agent

一个个人使用的 Rust Agent：支持 ReAct、多模态（截图/图片）、Python 脚本技能执行、对话记忆压缩（memory.md），并可对接钉钉/Telegram 用消息控制。当前已适配 OpenAI Chat Completions 与豆包/火山 Ark Responses 两类接口（按 base\_url 自动路由）。

## 项目结构

关键文件与目录：
- src/main.rs：程序入口，CLI 参数解析，启动 Agent/记忆/渠道与可选功能
- src/react.rs：ReAct 循环，工具调用协议，自动反馈/硬判定/Trace 日志
- src/llm.rs：模型客户端与路由（OpenAI Chat Completions / Ark Responses）
- src/tools.rs：工具实现（文件/Python/UI/截图/OCR/模板匹配/DOM 评估/pip_install）
- src/screenshot.rs：截图采集（不缩放）+ PNG 编码
- src/memory.rs：记忆（memory.jsonl）与压缩摘要（memory.md）
- src/upload.rs：上传客户端（可选，用于 URL 传图模式）
- src/dingtalk.rs：钉钉回调控制与 webhook 回复
- src/telegram.rs：Telegram long polling 控制
- agent.toml：示例配置（模型、python、记忆、渠道）
- persona.md：人格提示词
- cmd/upload-service：本地上传服务（适配 UploadClient 协议）

推荐阅读顺序：
- 先看 src/main.rs 和 src/react.rs，理解执行主流程与工具协议
- 再看 src/tools.rs，了解有哪些动作与评估器能力

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
- 截图会在本地截取后直接编码为 PNG（不缩放）
- 图片以 `data:image/png;base64,...` 形式内联到请求里（image_url）
- 截图分辨率与实际屏幕一致，可直接按截图坐标执行 ui_*

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

执行日志（Trace）：
- `--trace true|false`：是否输出执行过程日志（默认 true，输出到 stderr）
- `--trace-llm-raw-on-parse-error true|false`：当模型输出不是 JSON 时打印预览（默认 true）
- `--trace-max-preview-chars 400`：日志里预览最大字符数（默认 400）

## 工具与能力概览

Agent 以结构化 JSON 调工具，核心工具包括：

- 文件：read\_file / write\_file / list\_dir
- Python：run\_python（在指定 venv 内执行模型生成脚本）、pip\_install（给 venv 装依赖）
- UI 自动化：ui\_move / ui\_mouse\_down / ui\_mouse\_up / ui\_click / ui\_drag / ui\_type / ui\_key\_down / ui\_key\_up / ui\_keypress / ui\_scroll / sleep\_ms
  - ui\_click/button 支持：left/right/middle/back/forward
  - ui\_keypress/key 支持常用键名：enter/tab/esc/backspace/space/insert/delete/home/end/pgup/pgdn/capslock/numlock/scrolllock/pause/print/printscreen(prtsc)/apps(menu)、方向键 up/down/left/right、F1-F24、numpad0-9、numpad\_add/\_subtract/\_multiply/\_divide/\_decimal
  - 其它单字符按键直接用 key="a"、key="1"、key="." 等
- 视觉：capture\_screen（返回图片 data url；截图不缩放）
- 结构化评估器：
  - OCR：ocr\_screen / eval\_screen\_text
  - 模板匹配：match\_template（OpenCV）
  - DOM 评估：eval\_dom（Python Playwright）

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

如果你后续遇到某些模型/网关不接受 data-url，才需要切换为“先上传拿 URL，再传 image\_url”的模式（届时会用到 upload-service / S3 / 反向代理等）。

启动参数（或环境变量）：

```bash
cargo run -- --config agent.toml --auto-feedback --upload-endpoint http://127.0.0.1:9000/upload
```

环境变量：

- SELF\_AGENT\_UPLOAD\_ENDPOINT
- SELF\_AGENT\_UPLOAD\_API\_KEY（可选，Bearer）

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

你可以配置“硬判定规则”，让 Agent 在执行完 `ui_*` 工具后自动跑一次结构化评估（eval\_dom / match\_template），并把评估结果以 `HARD_CHECK` 消息注入上下文。模型应优先根据 `HARD_CHECK.verdict.ok` 决定是否 final。

### 方式 A：DOM 硬判定（网页更稳）

```bash
cargo run -- --config agent.toml ^
  --hard-dom-url "https://example.com" ^
  --hard-dom-selector "#submit" ^
  --hard-dom-timeout-ms 30000
```

规则：

- eval\_dom 返回 `ok=true`
- 若返回 `exists` 字段，则要求 `exists=true`

### 方式 B：模板匹配硬判定（桌面/固定 UI）

```bash
cargo run -- --config agent.toml ^
  --hard-template-path "templates/ok.png" ^
  --hard-template-threshold 0.85
```

规则：

- match\_template 返回 `ok=true` 且 `hit=true`

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

### 自动安装依赖（pip\_install）

Agent 现在支持通过工具 `pip_install` 给当前 venv 自动安装依赖（等价于 `python -m pip install ...`）。

典型用法（让模型先装依赖再调用评估器）：

- 安装 OpenCV 模板匹配依赖：
  - `opencv-python` / `numpy`
- 安装 Playwright（DOM 评估）依赖：
  - `playwright`（并且通常还需要执行 `playwright install` 下载浏览器）

注意：

- pip\_install 会在你配置的 `python.venv_path` 指向的解释器里执行
- 该操作会改动你的 venv，请在可控环境使用

pip\_install 工具入参示例（JSON）：

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

### 1) 模型看不到图片 / image\_url 无效

当前默认使用 `data:image/png;base64,...` 作为 image\_url（截图不缩放）。

如果模型/网关不支持 data-url：
- 需要切换为“先上传拿 URL，再传 image\_url”的模式（使用 upload-service / S3 / 反向代理等），并确保 URL 对模型可访问（不要是 `127.0.0.1`）

### 2) OCR / 模板匹配 / eval\_dom 报依赖缺失

- 先用 pip\_install 安装依赖：
  - OpenCV：`opencv-python numpy`
  - Playwright：`playwright` + 执行 `playwright install`
  - OCR：`pillow pytesseract`（并安装 tesseract）或 `easyocr`
说明：run\_python 遇到 `ModuleNotFoundError` 时，程序会尝试自动执行一次 pip\_install 并重跑（每次 turn 最多重试 2 次）；仍失败时再手动处理。

### 3) UI 自动化无效/点错

- UI 工具基于系统输入注入，受焦点窗口、权限、缩放/DPI 影响很大
- 建议配合 `capture_screen` / `ocr_screen` / `match_template` 做闭环确认

### 4) base\_url 与接口不匹配

- OpenAI：`https://api.openai.com`
- Ark Responses：`https://ark.../api/v3/responses`（当前实现会按 base\_url 自动路由）

### 5) run\_python / pip\_install 提示找不到 Python

检查 `agent.toml` 里的 `python.venv_path`，支持以下写法：
- 直接指向解释器：`.../python.exe`
- 指向 conda 环境目录（根目录含 python.exe）：`.../envs/xxx/`
- 指向 venv 目录（程序会尝试 `Scripts/python.exe`）：`.../venv/`

### 5) Trace 里出现 llm\_parse\_error

含义：模型输出的不是合法 JSON（常见原因是 JSON 字符串里出现了未转义的换行）。

处理：
- 程序会自动修复/提取一部分常见错误：
  - llm\_parse\_repaired：修复了字符串里的未转义换行/控制字符
  - llm\_parse\_extracted：从模型输出中提取出 JSON 部分并继续执行
- 如果仍然失败，建议在 prompt 里强调：JSON 字符串内使用 `\\n`，不要输出真实换行

## 安全与注意事项

- UI 自动化（enigo）会对真实系统发送鼠标键盘事件：建议在可控环境中使用，避免误操作。
- `run_python` 会执行模型生成的脚本：建议默认在隔离的工作目录使用，避免脚本误删/泄露本机数据。
- 不要把密钥写进公开仓库。更推荐使用环境变量注入。
