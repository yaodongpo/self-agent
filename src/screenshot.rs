use anyhow::{Context, Result};
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder};
use screenshots::Screen;

pub fn capture_primary_png() -> Result<Vec<u8>> {
    let screens = Screen::all().context("获取屏幕列表失败")?;
    let screen = screens.first().context("未找到可用屏幕")?;
    let shot = screen.capture().context("屏幕截图失败")?;

    let width = shot.width();
    let height = shot.height();
    let mut raw = shot.into_raw();
    for px in raw.chunks_exact_mut(4) {
        px.swap(0, 2);
    }

    let mut out = Vec::new();
    let encoder = PngEncoder::new(&mut out);
    encoder
        .write_image(&raw, width, height, ColorType::Rgba8.into())
        .context("编码PNG失败")?;
    Ok(out)
}
