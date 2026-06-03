pub mod cache;
mod encode;
mod error;
mod render;
pub mod score_style;
mod style;

use base64::Engine;
use image::{imageops, GenericImageView};
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

const MAX_CONCURRENT_RENDERS: usize = 1;

static RENDER_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn render_semaphore() -> &'static Semaphore {
    RENDER_SEMAPHORE.get_or_init(|| Semaphore::new(MAX_CONCURRENT_RENDERS))
}

fn extract_panic_message(e: tokio::task::JoinError) -> String {
    if e.is_panic() {
        let payload = e.into_panic();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        format!("render panicked: {msg}")
    } else {
        e.to_string()
    }
}

pub fn crop_and_resize(
    img: &image::DynamicImage,
    target_w: u32,
    target_h: u32,
) -> image::DynamicImage {
    let (w, h) = img.dimensions();
    let target_ratio = target_w as f64 / target_h as f64;
    let current_ratio = w as f64 / h as f64;

    let cropped = if current_ratio > target_ratio {
        let new_w = (h as f64 * target_ratio) as u32;
        let left = (w - new_w) / 2;
        img.crop_imm(left, 0, new_w, h)
    } else {
        let new_h = (w as f64 / target_ratio) as u32;
        let top = (h - new_h) / 2;
        img.crop_imm(0, top, w, new_h)
    };

    cropped.resize_exact(target_w, target_h, imageops::FilterType::Lanczos3)
}

pub fn blur_image(img: &image::DynamicImage, radius: u32) -> image::DynamicImage {
    let rgb = img.to_rgb8();
    let blurred = imageops::blur(&rgb, radius as f32);
    image::DynamicImage::ImageRgb8(blurred)
}

pub fn extract_dominant_hue(img: &image::DynamicImage) -> (u16, u16) {
    let small = img.resize_exact(32, 32, imageops::FilterType::Nearest);
    let rgb = small.to_rgb8();

    let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
    let pixels: Vec<_> = rgb.pixels().collect();
    let count = pixels.len() as u64;

    for p in &pixels {
        r_sum += p[0] as u64;
        g_sum += p[1] as u64;
        b_sum += p[2] as u64;
    }

    let r = r_sum as f64 / count as f64 / 255.0;
    let g = g_sum as f64 / count as f64 / 255.0;
    let b = b_sum as f64 / count as f64 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min) < 0.001 {
        return (200, 30); // grey fallback — cyan-blue is more neutral than red
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if (max - r).abs() < f64::EPSILON {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) * 60.0
    } else if (max - g).abs() < f64::EPSILON {
        ((b - r) / d + 2.0) * 60.0
    } else {
        ((r - g) / d + 4.0) * 60.0
    };

    let sat = (s * 100.0).clamp(25.0, 80.0) as u16;
    let hue = (h as u16) % 360;

    (hue, sat)
}

pub fn image_to_data_uri(img: &image::DynamicImage, quality: u8) -> Result<String, RenderError> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    rgb.write_with_encoder(encoder)
        .map_err(|e| RenderError::Encode(format!("JPEG encode: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

pub struct ScoreCardParams<'a> {
    pub score: &'a osubot_types::Score,
    pub username: &'a str,
    pub mode: osubot_types::GameMode,
    pub user_pp: f64,
    pub user_global_rank: Option<i64>,
    pub user_country_rank: Option<i64>,
    pub country_code: &'a str,
    pub avatar_url: &'a str,
    pub play_time: &'a str,
    pub fav_count: Option<i64>,
    pub play_count: Option<i64>,
    pub pp_change: Option<f64>,
    pub global_rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
    pub ranked_status: &'a str,
    pub ur_value: Option<f64>,
    pub ar_eff: Option<f64>,
    pub od_eff: Option<f64>,
    pub cs_eff: Option<f64>,
    pub hp_eff: Option<f64>,
    pub cover_image: Option<image::DynamicImage>,
    pub cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
}

pub async fn render_score_card(params: ScoreCardParams<'_>) -> Result<Vec<u8>, RenderError> {
    let ScoreCardParams {
        score,
        username,
        mode,
        user_pp,
        user_global_rank,
        user_country_rank,
        country_code,
        avatar_url,
        play_time,
        fav_count,
        play_count,
        pp_change,
        global_rank_change,
        country_rank_change,
        ranked_status,
        ur_value,
        ar_eff,
        od_eff,
        cs_eff,
        hp_eff,
        cover_image: preloaded_cover_image,
        cancel_flag: external_cancel,
    } = params;
    let client = cache::http_client();

    tracing::debug!("Downloading avatar: {}", avatar_url);
    let avatar_bytes = if !avatar_url.is_empty() {
        match cache::fetch_and_cache(avatar_url, client).await {
            Ok((bytes, _, _)) => bytes,
            Err(e) => {
                tracing::warn!("Avatar download failed: {}", e);
                vec![]
            }
        }
    } else {
        vec![]
    };
    tracing::debug!("Avatar downloaded: {} bytes", avatar_bytes.len());

    tracing::debug!("Preprocessing cover image...");
    let (bg_uri, thumb_uri, hue, sat) = if let Some(ref img) = preloaded_cover_image {
        let bg = crop_and_resize(img, 2560, 1440);
        let bg_uri = image_to_data_uri(&bg, 85)?;

        let thumb = crop_and_resize(img, 536, 300);
        let thumb_uri = image_to_data_uri(&thumb, 90)?;

        let (h, s) = extract_dominant_hue(img);

        (bg_uri, thumb_uri, h, s)
    } else {
        (String::new(), String::new(), 255, 30)
    };

    let avatar_uri = if !avatar_bytes.is_empty() {
        let img = image::load_from_memory(&avatar_bytes)
            .map_err(|e| RenderError::Render(format!("avatar decode: {e}")))?;
        let resized = img.resize_exact(116, 116, imageops::FilterType::Lanczos3);
        image_to_data_uri(&resized, 85)?
    } else {
        String::new()
    };

    let data = score_style::ScoreCardData {
        score: score.clone(),
        username: username.to_string(),
        mode,
        user_pp,
        user_global_rank,
        user_country_rank,
        country_code: country_code.to_string(),
        avatar_data_uri: avatar_uri,
        bg_data_uri: bg_uri,
        thumb_data_uri: thumb_uri,
        play_time: play_time.to_string(),
        hue,
        sat,
        fav_count,
        play_count,
        pp_change,
        global_rank_change,
        country_rank_change,
        ranked_status: ranked_status.to_string(),
        ur_value,
        ar_eff,
        od_eff,
        cs_eff,
        hp_eff,
    };

    let html = score_style::wrap_score_html(&data);
    tracing::debug!("HTML generated, starting render...");

    let _permit = render_semaphore()
        .acquire()
        .await
        .expect("render semaphore never closed");
    let font_ctx = get_font_context();
    let cancel =
        external_cancel.unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let cancel_flag = cancel.clone();

    let render_result = tokio::task::spawn_blocking(move || {
        render::render_html_to_image(&html, font_ctx, 2560, 1440, &cancel_flag)
    })
    .await;

    let (pixels, w, h) = match render_result {
        Ok(result) => result?,
        Err(e) => return Err(RenderError::Render(extract_panic_message(e))),
    };

    let jpeg = encode::encode_jpeg(pixels, w, h, 90).await?;
    Ok(jpeg)
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
    let render_result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::task::spawn_blocking(move || {
            render::render_html_to_image(&wrapped_html, font_ctx, width, height, &cancel_flag)
        }),
    )
    .await;

    let (mut pixels, mut w, mut h) = match render_result {
        Ok(Ok(result)) => result?,
        Ok(Err(e)) => return Err(RenderError::Render(extract_panic_message(e))),
        Err(_) => {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("render timed out after 60s, background task may still be running");
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
