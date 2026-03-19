use anyhow::{Context, Result};
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::ImageBuffer;
use image::RgbaImage;
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

    let max_side = 720u32;
    let (out_w, out_h, out_raw) = if width > max_side || height > max_side {
        let (new_w, new_h) = if width >= height {
            let w = max_side;
            let h = (height as u64 * max_side as u64 / width as u64).max(1) as u32;
            (w, h)
        } else {
            let h = max_side;
            let w = (width as u64 * max_side as u64 / height as u64).max(1) as u32;
            (w, h)
        };
        let img: RgbaImage = ImageBuffer::from_raw(width, height, raw).context("构建截图ImageBuffer失败")?;
        let resized = image::imageops::resize(&img, new_w, new_h, FilterType::Lanczos3);
        (new_w, new_h, resized.into_raw())
    } else {
        (width, height, raw)
    };

    let mut out = Vec::new();
    let encoder = PngEncoder::new(&mut out);
    encoder
        .write_image(&out_raw, out_w, out_h, ColorType::Rgba8)
        .context("编码PNG失败")?;
    Ok(out)
}
