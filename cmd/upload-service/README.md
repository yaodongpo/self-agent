# upload-service

一个最小可用的本地上传服务，用于把 `data_base64` 图片落盘并返回可访问的 URL，适配本仓库 `UploadClient` 的上传协议。

## 协议

- POST `/upload`
  - 请求 JSON：
    - `mime`: 例如 `image/png`
    - `data_base64`: base64 内容（不带 data url 头）
  - 响应 JSON：
    - `url`: 例如 `http://127.0.0.1:9000/files/xxxx.png`

- GET `/files/<name>`
  - 返回静态文件

- GET `/health`
  - 返回 `{"ok":true}`

## 运行

默认监听 `127.0.0.1:9000`，上传目录为 `./uploads`：

```bash
python server.py
```

自定义端口与 public base：

```bash
python server.py --host 0.0.0.0 --port 9000 --public-base http://127.0.0.1:9000
```

启用 token 鉴权（与 self-agent 的 `--upload-api-key` 对应 Bearer）：

```bash
python server.py --token YOUR_TOKEN
```

## self-agent 对接

启动 self-agent 时指定 upload endpoint：

```bash
cargo run -- --config agent.toml --upload-endpoint http://127.0.0.1:9000/upload --upload-api-key YOUR_TOKEN
```

