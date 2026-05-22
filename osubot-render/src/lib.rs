mod encode;
mod error;
mod render;
mod style;

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

    let (pixels, w, h) = tokio::task::spawn_blocking(move || {
        render::render_html_to_image(&wrapped_html, font_ctx, width, height)
    })
    .await
    .map_err(|e| RenderError::Render(e.to_string()))??;

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
