mod cache;
mod convert;
mod encode;
mod error;
pub mod font;
mod style;

use image::imageops;
use takumi::layout::style::StyleSheet;
use takumi::layout::Viewport;
use takumi::rendering::RenderOptions;

pub use cache::{cleanup_expired, ensure_cache_dir};
pub use error::RenderError;

pub const PROFILE_VIEWPORT_WIDTH: u32 = 1650;
pub const MAX_HTML_BYTES: usize = 4 * 1024 * 1024; // 4MB

const MAX_PHYSICAL_HEIGHT: u32 = 24000;

pub async fn render_profile_card(
    html: &str,
    profile_hue: u16,
    width: u32,
) -> Result<Vec<u8>, RenderError> {
    if html.len() > MAX_HTML_BYTES {
        return Err(RenderError::TooLarge(format!(
            "HTML input {} bytes exceeds limit of {} bytes",
            html.len(),
            MAX_HTML_BYTES
        )));
    }
    let html = cache::inline_external_images(html).await;
    let node = convert::html_to_node(&html)?;
    let stylesheet = StyleSheet::parse_loosy(&style::osu_web_stylesheet(profile_hue));

    let image = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            takumi::rendering::render(
                RenderOptions::builder()
                    .node(node)
                    .viewport(Viewport::new((width, None::<u32>)))
                    .stylesheet(stylesheet)
                    .global(font::get())
                    .build(),
            )
        }),
    )
    .await
    .map_err(|_| RenderError::Render("render timed out after 30s".into()))?
    .map_err(|e| RenderError::Render(e.to_string()))?
    .map_err(|e| RenderError::Render(e.to_string()))?;

    let (w, h) = (image.width(), image.height());

    let (pixels, w, h) = if h > MAX_PHYSICAL_HEIGHT {
        let scale = MAX_PHYSICAL_HEIGHT as f64 / h as f64;
        let new_w = (w as f64 * scale) as u32;
        let new_h = MAX_PHYSICAL_HEIGHT;
        let scaled = imageops::resize(&image, new_w, new_h, imageops::FilterType::Lanczos3);
        (scaled.into_raw(), new_w, new_h)
    } else {
        (image.into_raw(), w, h)
    };

    encode::encode_jpeg(pixels, w, h, 80).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stylesheet_has_correct_hue() {
        let css = style::osu_web_stylesheet(42);
        assert!(css.contains("--base-hue: 42"));
    }

    #[test]
    fn test_convert_preserves_content() {
        let node = convert::html_to_node(r#"<div class="bbcode"><p>hello</p></div>"#).unwrap();
        let dbg = format!("{:?}", node);
        assert!(dbg.contains("hello"));
    }

    #[tokio::test]
    async fn test_render_profile_card_smoke() {
        font::init();
        let result = render_profile_card(r#"<div class="bbcode">Hello</div>"#, 333, 800).await;
        assert!(result.is_ok());
        let jpeg = result.unwrap();
        assert!(!jpeg.is_empty());
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }

    #[tokio::test]
    async fn test_render_profile_card_realistic_html() {
        font::init();
        let html = r#"<div class="bbcode">
<h2>Hello!</h2>
<p>Welcome to my profile page.</p>
<div class="bbcode-spoilerbox">
    <a class="bbcode-spoilerbox__link">Show more</a>
    <div class="bbcode-spoilerbox__body"><p>Hidden content here</p></div>
</div>
<div class="well"><p>This is a well section.</p></div>
<blockquote><p>Quoted text</p></blockquote>
<p class="bbcode__align-centre">Centered text</p>
<img src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7" class="badge">
</div>"#;
        let result = render_profile_card(html, 200, PROFILE_VIEWPORT_WIDTH).await;
        assert!(result.is_ok());
        let jpeg = result.unwrap();
        assert!(!jpeg.is_empty());
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }

    #[tokio::test]
    async fn test_render_rejects_oversized_html() {
        font::init();
        let huge_html = "x".repeat(MAX_HTML_BYTES + 1);
        let result = render_profile_card(&huge_html, 333, 800).await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("Input too large"),
            "expected TooLarge error, got: {}",
            err
        );
    }
}
