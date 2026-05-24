mod cache;
mod encode;
mod error;
mod render;
mod style;

use image::imageops;
use parley::FontContext;
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

pub use cache::{cleanup_expired, ensure_cache_dir};
pub use error::RenderError;

pub const PROFILE_VIEWPORT_WIDTH: u32 = 1650;

static FONT_CTX: OnceLock<FontContext> = OnceLock::new();

fn get_font_context() -> &'static FontContext {
    FONT_CTX.get_or_init(FontContext::new)
}

/// Maximum concurrent render operations. Render is CPU-intensive (font rasterization,
/// layout, paint), so this limits parallel renders to avoid saturating CPU cores.
const MAX_CONCURRENT_RENDERS: usize = 1;

static RENDER_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn render_semaphore() -> &'static Semaphore {
    RENDER_SEMAPHORE.get_or_init(|| Semaphore::new(MAX_CONCURRENT_RENDERS))
}

pub async fn render_profile_card(
    html: &str,
    profile_hue: u16,
    avatar_url: &str,
    username: &str,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, RenderError> {
    let html_with_inlined_images = cache::inline_external_images(html).await;
    let _permit = render_semaphore()
        .acquire()
        .await
        .expect("render semaphore never closed");
    let wrapped_html =
        style::wrap_osu_profile_html(&html_with_inlined_images, profile_hue, avatar_url, username);
    let font_ctx = get_font_context();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_flag = cancel.clone();
    // 60s timeout: profile cards with many badges or large user stats can
    // take significant time to render, especially under concurrent load.
    // On timeout, the cancel flag is set so the blocking task exits quickly.
    let render_result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::task::spawn_blocking(move || {
            render::render_html_to_image(&wrapped_html, font_ctx, width, height, &cancel_flag)
        }),
    )
    .await;

    let (mut pixels, mut w, mut h) = match render_result {
        Ok(Ok(result)) => result?,
        Ok(Err(e)) => {
            if e.is_panic() {
                let panic_payload = e.into_panic();
                let panic_msg = panic_payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                return Err(RenderError::Render(format!("render panicked: {panic_msg}")));
            }
            return Err(RenderError::Render(e.to_string()));
        }
        Err(_) => {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            return Err(RenderError::Render("render timed out after 60s".into()));
        }
    };

    const MAX_PHYSICAL_HEIGHT: u32 = 24000;
    if h > MAX_PHYSICAL_HEIGHT {
        let scale = MAX_PHYSICAL_HEIGHT as f64 / h as f64;
        let new_w = (w as f64 * scale) as u32;
        let new_h = (h as f64 * scale) as u32;
        let expected = (w as usize) * (h as usize) * 4;
        let got = pixels.len();
        let img = image::RgbaImage::from_raw(w, h, pixels).ok_or_else(|| {
            RenderError::Encode(format!(
                "bad buffer for rescale: expected {} bytes ({}x{}), got {}",
                expected, w, h, got
            ))
        })?;
        let scaled = imageops::resize(&img, new_w, new_h, imageops::FilterType::Lanczos3);
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
    #[ignore = "requires display server for font rendering; run with --ignored"]
    async fn test_render_profile_card_smoke() {
        let html = r#"<div class="bbcode">Hello <strong>World</strong></div>"#;
        let result = render_profile_card(html, 333, "", "test", PROFILE_VIEWPORT_WIDTH, 1200).await;
        assert!(result.is_ok());
        let jpeg = result.unwrap();
        assert!(!jpeg.is_empty());
        assert!(jpeg.len() > 200);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
