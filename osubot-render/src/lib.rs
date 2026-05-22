mod encode;
mod error;
mod render;
mod style;

use image::imageops;
use parley::FontContext;
use std::sync::OnceLock;

pub use error::RenderError;

static FONT_CTX: OnceLock<FontContext> = OnceLock::new();

fn get_font_context() -> &'static FontContext {
    FONT_CTX.get_or_init(FontContext::new)
}

pub async fn render_profile_card(
    html: &str,
    profile_hue: u16,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, RenderError> {
    let wrapped_html = style::wrap_osu_profile_html(html, profile_hue);
    let font_ctx = get_font_context();

    let (mut pixels, mut w, mut h) = tokio::task::spawn_blocking(move || {
        render::render_html_to_image(&wrapped_html, font_ctx, width, height)
    })
    .await
    .map_err(|e| RenderError::Render(e.to_string()))??;

    const MAX_PHYSICAL_HEIGHT: u32 = 24000;
    if h > MAX_PHYSICAL_HEIGHT {
        let scale = MAX_PHYSICAL_HEIGHT as f64 / h as f64;
        let new_w = (w as f64 * scale) as u32;
        let new_h = (h as f64 * scale) as u32;
        let img = image::RgbaImage::from_raw(w, h, pixels)
            .ok_or(RenderError::Encode("bad buffer".into()))?;
        let scaled = imageops::resize(&img, new_w, new_h,
            imageops::FilterType::Lanczos3);
        pixels = scaled.into_raw();
        w = new_w;
        h = new_h;
    }

    let jpeg = encode::encode_jpeg(pixels, w, h, 80).await?;

    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires GPU; run with --ignored"]
    async fn test_render_profile_card_smoke() {
        let html = r#"<div class="bbcode">Hello <strong>World</strong></div>"#;
        let result = render_profile_card(html, 333, 1650, 1200).await;
        assert!(result.is_ok());
        let jpeg = result.unwrap();
        assert!(!jpeg.is_empty());
        assert!(jpeg.len() > 200);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
