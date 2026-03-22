use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::screenshot;
use crate::upload::UploadClient;
use crate::image_transport::{image_url_from_bytes, ImageTransportMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub action: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

#[derive(Clone)]
pub struct ToolContext {
    pub python_venv_path: PathBuf,
    pub python_timeout_seconds: u64,
    pub workspace_root: PathBuf,
    pub uploader: Option<UploadClient>,
    pub image_transport: ImageTransportMode,
}

pub async fn execute_tool(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    match call.action.as_str() {
        "run_python" => run_python(call, ctx).await,
        "pip_install" => pip_install(call, ctx).await,
        "read_file" => read_file(call, ctx).await,
        "write_file" => write_file(call, ctx).await,
        "list_dir" => list_dir(call, ctx).await,
        "sleep_ms" => sleep_ms(call).await,
        "ui_get_cursor_pos" => ui_get_cursor_pos(call).await,
        "ui_get_screens" => ui_get_screens(call).await,
        "ui_get_active_window" => ui_get_active_window(call).await,
        "ui_move" => ui_move(call).await,
        "ui_mouse_down" => ui_mouse_down(call).await,
        "ui_mouse_up" => ui_mouse_up(call).await,
        "ui_click" => ui_click(call).await,
        "ui_drag" => ui_drag(call).await,
        "ui_type" => ui_type(call).await,
        "ui_key_down" => ui_key_down(call).await,
        "ui_key_up" => ui_key_up(call).await,
        "ui_keypress" => ui_keypress(call).await,
        "ui_scroll" => ui_scroll(call).await,
        "capture_screen" => capture_screen(call, ctx).await,
        "ocr_screen" => ocr_screen(call, ctx).await,
        "eval_screen_text" => eval_screen_text(call, ctx).await,
        "match_template" => match_template(call, ctx).await,
        "eval_dom" => eval_dom(call, ctx).await,
        other => Ok(format!("未知工具: {other}")),
    }
}

async fn ui_get_cursor_pos(_call: &ToolCall) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        let ok = unsafe { GetCursorPos(&mut pt) };
        if ok == 0 {
            anyhow::bail!("GetCursorPos 失败");
        }
        return Ok(serde_json::json!({ "x": pt.x, "y": pt.y }).to_string());
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("ui_get_cursor_pos 仅支持 Windows");
    }
}

async fn ui_get_screens(_call: &ToolCall) -> Result<String> {
    let screens = screenshots::Screen::all().context("获取屏幕列表失败")?;
    let items = screens
        .into_iter()
        .map(|s| {
            let d = s.display_info;
            serde_json::json!({
                "id": d.id,
                "x": d.x,
                "y": d.y,
                "width": d.width,
                "height": d.height,
                "scale_factor": d.scale_factor,
                "rotation": d.rotation,
                "frequency": d.frequency,
                "is_primary": d.is_primary,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "screens": items }).to_string())
}

async fn ui_get_active_window(_call: &ToolCall) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::System::ProcessStatus::GetProcessImageFileNameW;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        };

        let hwnd: HWND = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return Ok(serde_json::json!({ "ok": false }).to_string());
        }

        let len = unsafe { GetWindowTextLengthW(hwnd) };
        let title = if len > 0 {
            let mut buf = vec![0u16; (len as usize) + 1];
            let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
            wide_to_string(&buf[..(n.max(0) as usize)])
        } else {
            String::new()
        };

        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };

        let mut process_path = None;
        if pid != 0 {
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if !handle.is_null() {
                let mut buf = vec![0u16; 1024];
                let mut size: u32 = buf.len() as u32;
                let ok = unsafe { windows_sys::Win32::System::ProcessStatus::GetProcessImageFileNameW(handle, buf.as_mut_ptr(), buf.len() as u32) };
                if ok != 0 && size > 0 {
                    process_path = Some(wide_to_string(&buf[..(size as usize)]));
                }
                unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            }
        }

        return Ok(
            serde_json::json!({
                "ok": true,

                "hwnd": hwnd as isize,
                "pid": pid,
                "title": title,
                "process_path": process_path,
            })
            .to_string(),
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("ui_get_active_window 仅支持 Windows");
    }
}

fn wide_to_string(s: &[u16]) -> String {
    String::from_utf16_lossy(s)
        .trim_end_matches('\u{0}')
        .to_string()
}

async fn run_python(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        code: String,
        #[serde(default)]
        args: Vec<String>,
        timeout_seconds: Option<u64>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("run_python.input 解析失败")?;

    let python = resolve_venv_python(ctx.python_venv_path.as_path())?;
    let mut tmp = NamedTempFile::new().context("创建临时脚本失败")?;
    std::io::Write::write_all(&mut tmp, input.code.as_bytes()).context("写入临时脚本失败")?;
    let script_path = tmp.path().to_path_buf();

    let mut cmd = Command::new(python);
    cmd.arg(script_path);
    for a in input.args {
        cmd.arg(a);
    }
    cmd.current_dir(&ctx.workspace_root);

    let t = input.timeout_seconds.unwrap_or(ctx.python_timeout_seconds);
    let output = timeout(Duration::from_secs(t), cmd.output())
        .await
        .context("Python执行超时")?
        .context("启动Python失败")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = String::new();
    combined.push_str("exit_code=");
    combined.push_str(&output.status.code().unwrap_or(-1).to_string());
    combined.push('\n');
    if !stdout.is_empty() {
        combined.push_str("stdout:\n");
        combined.push_str(&stdout);
        if !stdout.ends_with('\n') {
            combined.push('\n');
        }
    }
    if !stderr.is_empty() {
        combined.push_str("stderr:\n");
        combined.push_str(&stderr);
        if !stderr.ends_with('\n') {
            combined.push('\n');
        }
    }

    Ok(truncate(combined, 12_000))
}

async fn pip_install(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        packages: Vec<String>,
        #[serde(default)]
        upgrade: bool,
        #[serde(default)]
        pre: bool,
        #[serde(default)]
        no_deps: bool,
        #[serde(default)]
        index_url: Option<String>,
        #[serde(default)]
        extra_index_url: Option<String>,
        #[serde(default)]
        timeout_seconds: Option<u64>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("pip_install.input 解析失败")?;
    if input.packages.is_empty() {
        anyhow::bail!("pip_install.packages 不能为空");
    }
    for p in &input.packages {
        if p.trim().is_empty() {
            anyhow::bail!("pip_install.packages 包含空字符串");
        }
        if p.contains('\n') || p.contains('\r') {
            anyhow::bail!("pip_install.packages 不允许换行");
        }
    }

    let python = resolve_venv_python(ctx.python_venv_path.as_path())?;
    let mut cmd = Command::new(python);
    cmd.arg("-m").arg("pip").arg("install");
    if input.upgrade {
        cmd.arg("--upgrade");
    }
    if input.pre {
        cmd.arg("--pre");
    }
    if input.no_deps {
        cmd.arg("--no-deps");
    }
    if let Some(u) = input.index_url {
        if !u.trim().is_empty() {
            cmd.arg("--index-url").arg(u);
        }
    }
    if let Some(u) = input.extra_index_url {
        if !u.trim().is_empty() {
            cmd.arg("--extra-index-url").arg(u);
        }
    }
    for p in input.packages {
        cmd.arg(p);
    }
    cmd.current_dir(&ctx.workspace_root);

    let t = input.timeout_seconds.unwrap_or(ctx.python_timeout_seconds).max(30);
    let output = timeout(Duration::from_secs(t), cmd.output())
        .await
        .context("pip_install 执行超时")?
        .context("启动 pip_install 失败")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = String::new();
    combined.push_str("exit_code=");
    combined.push_str(&output.status.code().unwrap_or(-1).to_string());
    combined.push('\n');
    if !stdout.is_empty() {
        combined.push_str("stdout:\n");
        combined.push_str(&stdout);
        if !stdout.ends_with('\n') {
            combined.push('\n');
        }
    }
    if !stderr.is_empty() {
        combined.push_str("stderr:\n");
        combined.push_str(&stderr);
        if !stderr.ends_with('\n') {
            combined.push('\n');
        }
    }
    Ok(truncate(combined, 24_000))
}

async fn sleep_ms(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        ms: u64,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("sleep_ms.input 解析失败")?;
    tokio::time::sleep(Duration::from_millis(input.ms)).await;
    Ok("ok".to_string())
}

async fn ui_move(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        x: i32,
        y: i32,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_move.input 解析失败")?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo
            .move_mouse(input.x, input.y, Coordinate::Abs)
            .context("移动鼠标失败")?;
        Ok(())
    })
    .await
    .context("ui_move 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_mouse_down(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        button: Option<String>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_mouse_down.input 解析失败")?;
    let button = parse_mouse_button(input.button.as_deref());
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo.button(button, Direction::Press).context("鼠标按下失败")?;
        Ok(())
    })
    .await
    .context("ui_mouse_down 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_mouse_up(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        button: Option<String>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_mouse_up.input 解析失败")?;
    let button = parse_mouse_button(input.button.as_deref());
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo
            .button(button, Direction::Release)
            .context("鼠标抬起失败")?;
        Ok(())
    })
    .await
    .context("ui_mouse_up 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_click(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        x: i32,
        y: i32,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        clicks: Option<u32>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_click.input 解析失败")?;
    let button = parse_mouse_button(input.button.as_deref());
    let clicks = input.clicks.unwrap_or(1).clamp(1, 5);
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo
            .move_mouse(input.x, input.y, Coordinate::Abs)
            .context("移动鼠标失败")?;
        for _ in 0..clicks {
            enigo.button(button, Direction::Click).context("鼠标点击失败")?;
        }
        Ok(())
    })
    .await
    .context("ui_click 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_drag(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        #[serde(default)]
        button: Option<String>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_drag.input 解析失败")?;
    let button = parse_mouse_button(input.button.as_deref());
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo
            .move_mouse(input.from_x, input.from_y, Coordinate::Abs)
            .context("移动鼠标失败")?;
        enigo.button(button, Direction::Press).context("鼠标按下失败")?;
        enigo
            .move_mouse(input.to_x, input.to_y, Coordinate::Abs)
            .context("移动鼠标失败")?;
        enigo
            .button(button, Direction::Release)
            .context("鼠标抬起失败")?;
        Ok(())
    })
    .await
    .context("ui_drag 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_type(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        text: String,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_type.input 解析失败")?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo.text(&input.text).context("输入文本失败")?;
        Ok(())
    })
    .await
    .context("ui_type 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_key_down(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        key: String,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_key_down.input 解析失败")?;
    let key = parse_key(&input.key).context("不支持的key")?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo.key(key, Direction::Press).context("按键按下失败")?;
        Ok(())
    })
    .await
    .context("ui_key_down 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_key_up(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        key: String,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_key_up.input 解析失败")?;
    let key = parse_key(&input.key).context("不支持的key")?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        enigo
            .key(key, Direction::Release)
            .context("按键抬起失败")?;
        Ok(())
    })
    .await
    .context("ui_key_up 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_keypress(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_keypress.input 解析失败")?;
    let key = parse_key(&input.key).context("不支持的key")?;
    let modifiers = input.modifiers;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        let mut mods = modifiers;
        mods.sort();
        mods.dedup();
        for m in &mods {
            if let Some(k) = parse_modifier(m) {
                enigo.key(k, Direction::Press).context("按下修饰键失败")?;
            }
        }
        enigo.key(key, Direction::Click).context("按键失败")?;
        for m in mods.iter().rev() {
            if let Some(k) = parse_modifier(m) {
                enigo.key(k, Direction::Release).context("释放修饰键失败")?;
            }
        }
        Ok(())
    })
    .await
    .context("ui_keypress 线程失败")??;
    Ok("ok".to_string())
}

async fn ui_scroll(call: &ToolCall) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        delta_y: i32,
        #[serde(default)]
        delta_x: i32,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ui_scroll.input 解析失败")?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default()).context("创建Enigo失败")?;
        if input.delta_y != 0 {
            enigo
                .scroll(input.delta_y, Axis::Vertical)
                .context("滚动Y失败")?;
        }
        if input.delta_x != 0 {
            enigo
                .scroll(input.delta_x, Axis::Horizontal)
                .context("滚动X失败")?;
        }
        Ok(())
    })
    .await
    .context("ui_scroll 线程失败")??;
    Ok("ok".to_string())
}

async fn capture_screen(_call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    let png = tokio::task::spawn_blocking(screenshot::capture_primary_png)
        .await
        .context("截图任务失败")??;
    image_url_from_bytes(
        "image/png",
        &png,
        ctx.image_transport,
        ctx.uploader.clone(),
    )
    .await
}

async fn ocr_screen(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        lang: Option<String>,
        #[serde(default)]
        timeout_seconds: Option<u64>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("ocr_screen.input 解析失败")?;
    let png = tokio::task::spawn_blocking(screenshot::capture_primary_png)
        .await
        .context("截图任务失败")??;
    let mut tmp = NamedTempFile::new().context("创建临时图片失败")?;
    std::io::Write::write_all(&mut tmp, &png).context("写入临时图片失败")?;
    let img_path = tmp.path().to_path_buf();
    let lang = input.lang.unwrap_or_else(|| "eng".to_string());
    let t = input.timeout_seconds.unwrap_or(ctx.python_timeout_seconds);
    run_ocr_python(ctx, img_path, lang, t).await
}

async fn eval_screen_text(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        must_contain: Vec<String>,
        #[serde(default)]
        must_not_contain: Vec<String>,
        #[serde(default)]
        timeout_seconds: Option<u64>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("eval_screen_text.input 解析失败")?;
    let ocr = ocr_screen(
        &ToolCall {
            action: "ocr_screen".to_string(),
            input: serde_json::json!({ "timeout_seconds": input.timeout_seconds }),
        },
        ctx,
    )
    .await?;
    let lower = ocr.to_lowercase();
    let mut missing = Vec::new();
    for s in &input.must_contain {
        if !lower.contains(&s.to_lowercase()) {
            missing.push(s.clone());
        }
    }
    let mut hit_forbidden = Vec::new();
    for s in &input.must_not_contain {
        if lower.contains(&s.to_lowercase()) {
            hit_forbidden.push(s.clone());
        }
    }
    let ok = missing.is_empty() && hit_forbidden.is_empty();
    let res = serde_json::json!({
        "ok": ok,
        "missing": missing,
        "forbidden_hits": hit_forbidden,
        "ocr_len": ocr.len(),
        "ocr_head": ocr.chars().take(300).collect::<String>(),
    });
    Ok(res.to_string())
}

async fn match_template(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        template_path: String,
        #[serde(default)]
        threshold: Option<f32>,
        #[serde(default)]
        timeout_seconds: Option<u64>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("match_template.input 解析失败")?;
    let template_abs = safe_join(ctx.workspace_root.as_path(), &input.template_path)?;

    let png = tokio::task::spawn_blocking(screenshot::capture_primary_png)
        .await
        .context("截图任务失败")??;
    let mut tmp = NamedTempFile::new().context("创建临时截图失败")?;
    std::io::Write::write_all(&mut tmp, &png).context("写入临时截图失败")?;
    let screen_path = tmp.path().to_path_buf();

    let t = input.timeout_seconds.unwrap_or(ctx.python_timeout_seconds);
    run_template_match_python(ctx, screen_path, template_abs, input.threshold, t).await
}

async fn eval_dom(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        url: String,
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        js: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("eval_dom.input 解析失败")?;
    let timeout_ms = input.timeout_ms.unwrap_or(30_000);
    run_dom_eval_python(ctx, input.url, input.selector, input.js, timeout_ms).await
}

async fn read_file(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        path: String,
        #[serde(default)]
        max_bytes: Option<usize>,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("read_file.input 解析失败")?;
    let abs = safe_join(ctx.workspace_root.as_path(), &input.path)?;
    let max = input.max_bytes.unwrap_or(32_768);
    let bytes = tokio::fs::read(&abs).await.with_context(|| format!("读取文件失败: {}", abs.display()))?;
    let sliced = bytes.into_iter().take(max).collect::<Vec<_>>();
    Ok(String::from_utf8_lossy(&sliced).to_string())
}

async fn write_file(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        path: String,
        content: String,
        #[serde(default)]
        overwrite: bool,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("write_file.input 解析失败")?;
    let abs = safe_join(ctx.workspace_root.as_path(), &input.path)?;
    if abs.exists() && !input.overwrite {
        anyhow::bail!("文件已存在且overwrite=false: {}", abs.display());
    }
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    tokio::fs::write(&abs, input.content.as_bytes())
        .await
        .with_context(|| format!("写入文件失败: {}", abs.display()))?;
    Ok(format!("ok: {}", abs.display()))
}

async fn list_dir(call: &ToolCall, ctx: &ToolContext) -> Result<String> {
    #[derive(Deserialize)]
    struct Input {
        path: String,
    }
    let input: Input = serde_json::from_value(call.input.clone()).context("list_dir.input 解析失败")?;
    let abs = safe_join(ctx.workspace_root.as_path(), &input.path)?;
    let mut rd = tokio::fs::read_dir(&abs)
        .await
        .with_context(|| format!("读取目录失败: {}", abs.display()))?;
    let mut entries = Vec::new();
    while let Some(e) = rd.next_entry().await? {
        let ty = e.file_type().await?;
        let name = e.file_name().to_string_lossy().to_string();
        entries.push(if ty.is_dir() { format!("{name}/") } else { name });
    }
    entries.sort();
    Ok(entries.join("\n"))
}

fn resolve_venv_python(venv_path: &Path) -> Result<PathBuf> {
    if venv_path.is_file() {
        return Ok(venv_path.to_path_buf());
    }

    let candidates = vec![
        venv_path.join("python.exe"),
        venv_path.join("Scripts").join("python.exe"),
        venv_path.join("Scripts").join("pythonw.exe"),
        venv_path.join("bin").join("python"),
    ];

    for c in &candidates {
        if c.exists() {
            return Ok(c.to_path_buf());
        }
    }

    if let Some(p) = find_in_path("python.exe") {
        return Ok(p);
    }

    let attempted = candidates
        .into_iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::bail!("找不到Python可执行文件: venv_path={} attempted=[{}]", venv_path.display(), attempted)
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(exe);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

async fn run_ocr_python(ctx: &ToolContext, img_path: PathBuf, lang: String, timeout_seconds: u64) -> Result<String> {
    let code = r#"import json, sys
path = sys.argv[1]
lang = sys.argv[2] if len(sys.argv) > 2 else "eng"
def out(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False))
    sys.stdout.flush()
try:
    from PIL import Image
    img = Image.open(path)
except Exception as e:
    out({"ok": False, "error": "PIL not available: " + str(e)})
    sys.exit(0)
try:
    import pytesseract
    text = pytesseract.image_to_string(img, lang=lang)
    out({"ok": True, "text": text})
    sys.exit(0)
except Exception as e:
    err1 = str(e)
try:
    import easyocr
    reader = easyocr.Reader(["ch_sim","en"], gpu=False)
    res = reader.readtext(path, detail=0, paragraph=True)
    out({"ok": True, "text": "\n".join(res)})
    sys.exit(0)
except Exception as e2:
    out({"ok": False, "error": "OCR deps missing. Install pillow+pytesseract or easyocr. " + err1 + "; " + str(e2)})
    sys.exit(0)
"#;

    let python = resolve_venv_python(ctx.python_venv_path.as_path())?;
    let mut tmp = NamedTempFile::new().context("创建临时OCR脚本失败")?;
    std::io::Write::write_all(&mut tmp, code.as_bytes()).context("写入临时OCR脚本失败")?;
    let script_path = tmp.path().to_path_buf();

    let mut cmd = Command::new(python);
    cmd.arg(script_path);
    cmd.arg(img_path);
    cmd.arg(lang);
    cmd.current_dir(&ctx.workspace_root);

    let output = timeout(Duration::from_secs(timeout_seconds), cmd.output())
        .await
        .context("OCR执行超时")?
        .context("启动OCR Python失败")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if stdout.trim().is_empty() && !stderr.trim().is_empty() {
        return Ok(truncate(stderr, 12_000));
    }
    let v = serde_json::from_str::<serde_json::Value>(stdout.trim());
    match v {
        Ok(v) => {
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                return Ok(truncate(text, 24_000));
            }
            let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("unknown").to_string();
            Ok(truncate(err, 12_000))
        }
        Err(_) => Ok(truncate(stdout, 24_000)),
    }
}

async fn run_template_match_python(
    ctx: &ToolContext,
    screen_path: PathBuf,
    template_path: PathBuf,
    threshold: Option<f32>,
    timeout_seconds: u64,
) -> Result<String> {
    let code = r#"import json, sys
screen_path = sys.argv[1]
template_path = sys.argv[2]
threshold = float(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] != "" else None
def out(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False))
    sys.stdout.flush()

try:
    import cv2
    import numpy as np
except Exception as e:
    out({"ok": False, "error": "cv2/numpy not available. Install opencv-python numpy. " + str(e)})
    sys.exit(0)

screen = cv2.imread(screen_path, cv2.IMREAD_COLOR)
tpl = cv2.imread(template_path, cv2.IMREAD_COLOR)
if screen is None:
    out({"ok": False, "error": "failed to read screen image"})
    sys.exit(0)
if tpl is None:
    out({"ok": False, "error": "failed to read template image"})
    sys.exit(0)

sh, sw = screen.shape[:2]
th, tw = tpl.shape[:2]
if th > sh or tw > sw:
    out({"ok": False, "error": "template larger than screen", "screen": [sw, sh], "template": [tw, th]})
    sys.exit(0)

res = cv2.matchTemplate(screen, tpl, cv2.TM_CCOEFF_NORMED)
min_val, max_val, min_loc, max_loc = cv2.minMaxLoc(res)
score = float(max_val)
x, y = int(max_loc[0]), int(max_loc[1])
hit = True if threshold is None else (score >= threshold)
out({"ok": True, "hit": hit, "score": score, "x": x, "y": y, "w": int(tw), "h": int(th), "screen": [int(sw), int(sh)]})
"#;

    let python = resolve_venv_python(ctx.python_venv_path.as_path())?;
    let mut tmp = NamedTempFile::new().context("创建临时模板匹配脚本失败")?;
    std::io::Write::write_all(&mut tmp, code.as_bytes()).context("写入临时模板匹配脚本失败")?;
    let script_path = tmp.path().to_path_buf();

    let mut cmd = Command::new(python);
    cmd.arg(script_path);
    cmd.arg(screen_path);
    cmd.arg(template_path);
    cmd.arg(threshold.map(|t| t.to_string()).unwrap_or_default());
    cmd.current_dir(&ctx.workspace_root);

    let output = timeout(Duration::from_secs(timeout_seconds), cmd.output())
        .await
        .context("模板匹配执行超时")?
        .context("启动模板匹配 Python失败")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if stdout.trim().is_empty() && !stderr.trim().is_empty() {
        return Ok(truncate(stderr, 12_000));
    }
    Ok(truncate(stdout.trim().to_string(), 24_000))
}

async fn run_dom_eval_python(
    ctx: &ToolContext,
    url: String,
    selector: Option<String>,
    js: Option<String>,
    timeout_ms: u64,
) -> Result<String> {
    let code = r#"import json, sys
url = sys.argv[1]
selector = sys.argv[2] if len(sys.argv) > 2 and sys.argv[2] != "" else None
js = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] != "" else None
timeout_ms = int(sys.argv[4]) if len(sys.argv) > 4 and sys.argv[4] != "" else 30000
def out(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False))
    sys.stdout.flush()

try:
    from playwright.sync_api import sync_playwright
except Exception as e:
    out({"ok": False, "error": "playwright not available. Install playwright and run playwright install. " + str(e)})
    sys.exit(0)

try:
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        page = browser.new_page()
        page.goto(url, wait_until="domcontentloaded", timeout=timeout_ms)
        exists = None
        text = None
        count = None
        if selector:
            loc = page.locator(selector)
            count = loc.count()
            exists = (count > 0)
            if exists:
                try:
                    text = loc.first.inner_text(timeout=timeout_ms)
                except Exception:
                    text = None
        js_value = None
        if js:
            js_value = page.evaluate(js)
        browser.close()
        out({"ok": True, "url": url, "selector": selector, "exists": exists, "count": count, "text": text, "js_value": js_value})
except Exception as e:
    out({"ok": False, "error": str(e)})
"#;

    let python = resolve_venv_python(ctx.python_venv_path.as_path())?;
    let mut tmp = NamedTempFile::new().context("创建临时DOM评估脚本失败")?;
    std::io::Write::write_all(&mut tmp, code.as_bytes()).context("写入临时DOM评估脚本失败")?;
    let script_path = tmp.path().to_path_buf();

    let mut cmd = Command::new(python);
    cmd.arg(script_path);
    cmd.arg(url);
    cmd.arg(selector.unwrap_or_default());
    cmd.arg(js.unwrap_or_default());
    cmd.arg(timeout_ms.to_string());
    cmd.current_dir(&ctx.workspace_root);

    let output = timeout(Duration::from_secs(ctx.python_timeout_seconds), cmd.output())
        .await
        .context("DOM评估执行超时")?
        .context("启动DOM评估 Python失败")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if stdout.trim().is_empty() && !stderr.trim().is_empty() {
        return Ok(truncate(stderr, 12_000));
    }
    Ok(truncate(stdout.trim().to_string(), 24_000))
}

fn parse_mouse_button(button: Option<&str>) -> Button {
    match button.unwrap_or("left").to_ascii_lowercase().as_str() {
        "right" => Button::Right,
        "middle" => Button::Middle,
        "back" | "x1" | "xbutton1" => Button::Back,
        "forward" | "x2" | "xbutton2" => Button::Forward,
        "scrollup" => Button::ScrollUp,
        "scrolldown" => Button::ScrollDown,
        "scrollleft" => Button::ScrollLeft,
        "scrollright" => Button::ScrollRight,
        _ => Button::Left,
    }
}

fn parse_modifier(s: &str) -> Option<Key> {
    match s.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(Key::Control),
        "alt" => Some(Key::Alt),
        "shift" => Some(Key::Shift),
        "meta" | "win" | "super" => Some(Key::Meta),
        _ => None,
    }
}

fn parse_key(s: &str) -> Option<Key> {
    let k = s.to_ascii_lowercase();
    match k.as_str() {
        "enter" | "return" => Some(Key::Return),
        "tab" => Some(Key::Tab),
        "esc" | "escape" => Some(Key::Escape),
        "backspace" => Some(Key::Backspace),
        "space" => Some(Key::Space),
        "up" | "uparrow" => Some(Key::UpArrow),
        "down" | "downarrow" => Some(Key::DownArrow),
        "left" | "leftarrow" => Some(Key::LeftArrow),
        "right" | "rightarrow" => Some(Key::RightArrow),
        "delete" | "del" => Some(Key::Delete),
        "insert" | "ins" => Some(Key::Insert),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "capslock" => Some(Key::CapsLock),
        "numlock" => Some(Key::Numlock),
        "pause" => Some(Key::Pause),
        "print" => Some(Key::Print),
        "printscreen" | "prtsc" | "snapshot" => Some(Key::Snapshot),
        "apps" | "menu" | "contextmenu" => Some(Key::Apps),
        "browser_back" | "browserback" => Some(Key::BrowserBack),
        "browser_forward" | "browserforward" => Some(Key::BrowserForward),
        "browser_refresh" | "browserrefresh" => Some(Key::BrowserRefresh),
        "browser_stop" | "browserstop" => Some(Key::BrowserStop),
        "browser_home" | "browserhome" => Some(Key::BrowserHome),
        "volumeup" | "volume_up" => Some(Key::VolumeUp),
        "volumedown" | "volume_down" => Some(Key::VolumeDown),
        "volumemute" | "volume_mute" | "mute" => Some(Key::VolumeMute),
        "media_next" | "medianext" | "nexttrack" => Some(Key::MediaNextTrack),
        "media_prev" | "mediaprev" | "prevtrack" => Some(Key::MediaPrevTrack),
        "media_play_pause" | "mediaplaypause" | "playpause" => Some(Key::MediaPlayPause),
        "media_stop" | "mediastop" => Some(Key::MediaStop),
        "numpad0" | "kp0" => Some(Key::Numpad0),
        "numpad1" | "kp1" => Some(Key::Numpad1),
        "numpad2" | "kp2" => Some(Key::Numpad2),
        "numpad3" | "kp3" => Some(Key::Numpad3),
        "numpad4" | "kp4" => Some(Key::Numpad4),
        "numpad5" | "kp5" => Some(Key::Numpad5),
        "numpad6" | "kp6" => Some(Key::Numpad6),
        "numpad7" | "kp7" => Some(Key::Numpad7),
        "numpad8" | "kp8" => Some(Key::Numpad8),
        "numpad9" | "kp9" => Some(Key::Numpad9),
        "numpad_add" | "numpadadd" | "kp_add" => Some(Key::Add),
        "numpad_subtract" | "numpadsub" | "kp_subtract" => Some(Key::Subtract),
        "numpad_multiply" | "numpadmul" | "kp_multiply" => Some(Key::Multiply),
        "numpad_divide" | "numpaddiv" | "kp_divide" => Some(Key::Divide),
        "numpad_decimal" | "numpaddec" | "kp_decimal" => Some(Key::Decimal),
        "numpad_separator" | "kp_separator" => Some(Key::Separator),
        _ => {
            if let Some(rest) = k.strip_prefix("f") {
                if let Ok(n) = rest.parse::<u8>() {
                    return match n {
                        1 => Some(Key::F1),
                        2 => Some(Key::F2),
                        3 => Some(Key::F3),
                        4 => Some(Key::F4),
                        5 => Some(Key::F5),
                        6 => Some(Key::F6),
                        7 => Some(Key::F7),
                        8 => Some(Key::F8),
                        9 => Some(Key::F9),
                        10 => Some(Key::F10),
                        11 => Some(Key::F11),
                        12 => Some(Key::F12),
                        13 => Some(Key::F13),
                        14 => Some(Key::F14),
                        15 => Some(Key::F15),
                        16 => Some(Key::F16),
                        17 => Some(Key::F17),
                        18 => Some(Key::F18),
                        19 => Some(Key::F19),
                        20 => Some(Key::F20),
                        21 => Some(Key::F21),
                        22 => Some(Key::F22),
                        23 => Some(Key::F23),
                        24 => Some(Key::F24),
                        _ => None,
                    };
                }
            }
            if k.chars().count() == 1 {
                return k.chars().next().map(Key::Unicode);
            }
            None
        }
    }
}

fn safe_join(root: &Path, user_path: &str) -> Result<PathBuf> {
    let candidate = PathBuf::from(user_path);
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("不允许使用 .. 组件: {user_path}");
    }
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    };
    let root = root.canonicalize().context("workspace_root规范化失败")?;
    if !joined.starts_with(&root) {
        anyhow::bail!("路径越界: {user_path}");
    }
    Ok(joined)
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    s.truncate(max);
    s.push_str("\n...[truncated]\n");
    s
}

