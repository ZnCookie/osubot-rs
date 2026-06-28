pub mod cache;
mod encode;
mod error;
// ponytail: Task 4 adds the internal match template skeleton; Task 6 wires it
// into the public JPEG renderer.
pub mod match_style;
mod render;
pub mod score_list_style;
pub mod score_style;
pub mod style;
pub mod svg_css;
pub mod template;

use base64::Engine;
use image::{imageops, GenericImageView};
use osubot_core::log_fmt;
use parley::{fontique::Blob, FontContext};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex as StdMutex, OnceLock,
};

pub use cache::{cleanup_by_size, cleanup_expired, ensure_cache_dir};
pub use error::RenderError;
pub use render::render_html_to_image;

pub const PROFILE_VIEWPORT_WIDTH: u32 = 1000;

/// !ps / !rs 渲染超时（秒）。比单 score card 渲染慢：4 列网格 HTML 体积更大，blitz 布局耗时增加。
pub const SCORE_LIST_RENDER_TIMEOUT_SECS: u64 = 120;

static FONT_CTX: OnceLock<FontContext> = OnceLock::new();

const RENDER_FONT_DIRS_ENV: &str = "OSUBOT_RENDER_FONT_DIRS";
const MAX_FALLBACK_FONT_FILES: usize = 128;

fn get_font_context() -> &'static FontContext {
    FONT_CTX.get_or_init(|| {
        let mut font_ctx = FontContext::new();
        load_fallback_fonts_if_needed(&mut font_ctx);
        font_ctx
    })
}

fn load_fallback_fonts_if_needed(font_ctx: &mut FontContext) {
    let mut registered = 0;
    for dir in fallback_font_dirs() {
        register_fonts_from_dir(font_ctx, &dir, &mut registered);
        if registered >= MAX_FALLBACK_FONT_FILES {
            return;
        }
    }
}

fn fallback_font_dirs() -> Vec<PathBuf> {
    let mut dirs = std::env::var_os(RENDER_FONT_DIRS_ENV)
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();

    dirs.extend([
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/run/current-system/sw/share/fonts"),
    ]);

    dirs.extend(fontconfig_font_dirs());

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
    }

    dirs
}

/// Parse fontconfig configuration files for configured font directories.
///
/// This lets the renderer discover fonts on systems (such as NixOS) where
/// fonts live in store paths referenced by `/etc/fonts/conf.d/*.conf` instead
/// of the traditional `/usr/share/fonts` hierarchy.
fn fontconfig_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut conf_paths = vec![PathBuf::from("/etc/fonts/fonts.conf")];
    if let Ok(entries) = std::fs::read_dir("/etc/fonts/conf.d") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("conf") {
                conf_paths.push(path);
            }
        }
    }

    for conf_path in conf_paths {
        let Ok(content) = std::fs::read_to_string(&conf_path) else {
            continue;
        };
        for dir in parse_fontconfig_dir_tags(&content) {
            if dir.is_absolute() && dir.exists() && seen.insert(dir.clone()) {
                dirs.push(dir);
            }
        }
    }

    dirs
}

/// Extract text content of `<dir>` elements from a fontconfig XML file.
///
/// This is intentionally a minimal string scan rather than a full XML parse:
/// fontconfig configuration files only use `<dir>` to list font directories,
/// and we only keep absolute, existing paths.
fn parse_fontconfig_dir_tags(content: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut rest = content;

    while let Some(open_start) = rest.find("<dir") {
        rest = &rest[open_start + 4..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let after_open = &rest[tag_end + 1..];
        let Some(close_start) = after_open.find("</dir>") else {
            break;
        };
        let text = after_open[..close_start].trim();
        if !text.is_empty() {
            dirs.push(PathBuf::from(text));
        }
        rest = &after_open[close_start + 6..];
    }

    dirs
}

fn register_fonts_from_dir(font_ctx: &mut FontContext, dir: &Path, registered: &mut usize) {
    if *registered >= MAX_FALLBACK_FONT_FILES {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if *registered >= MAX_FALLBACK_FONT_FILES {
            return;
        }

        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            register_fonts_from_dir(font_ctx, &path, registered);
        } else if file_type.is_file() && is_font_file(&path) {
            register_font_file(font_ctx, &path, registered);
        }
    }
}

fn register_font_file(font_ctx: &mut FontContext, path: &Path, registered: &mut usize) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if !font_ctx
        .collection
        .register_fonts(Blob::from(bytes), None)
        .is_empty()
    {
        *registered += 1;
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
        .unwrap_or(false)
}

/// Global render mutex ensures only one render runs at a time.
/// Unlike tokio::sync::Semaphore (which releases on timeout even if the
/// spawn_blocking task is still running), a std::sync::Mutex held inside
/// the blocking closure stays locked until the task finishes — even after
/// a timeout cancels the outer async await.
static RENDER_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

fn render_lock() -> &'static StdMutex<()> {
    RENDER_LOCK.get_or_init(|| StdMutex::new(()))
}

fn locked_render() -> std::sync::MutexGuard<'static, ()> {
    render_lock().lock().unwrap_or_else(|e| e.into_inner())
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

async fn run_render(
    html: String,
    width: u32,
    height: u32,
    timeout_secs: u64,
    external_cancel: Option<Arc<AtomicBool>>,
) -> Result<(Vec<u8>, u32, u32), RenderError> {
    let cancel = external_cancel.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let handle = tokio::task::spawn_blocking({
        let cancel = cancel.clone();
        move || {
            let _guard = locked_render();
            let font_ctx = get_font_context();
            render::render_html_to_image(&html, font_ctx, width, height, &cancel)
        }
    });
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), handle).await {
        // 三层嵌套：timeout 成功 → JoinHandle 成功 → 渲染成功。
        Ok(Ok(Ok(r))) => Ok(r),
        // timeout 成功、JoinHandle 成功，但渲染本身返回错误。
        Ok(Ok(Err(e))) => Err(e),
        // timeout 成功，但 spawn_blocking 任务 panic。
        Ok(Err(e)) => Err(RenderError::Panicked(extract_panic_message(e))),
        // 渲染超时：通知取消标志并返回 Timeout。
        Err(_) => {
            cancel.store(true, Ordering::SeqCst);
            Err(RenderError::Timeout)
        }
    }
}

#[must_use]
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
        let left = w.saturating_sub(new_w) / 2;
        img.crop_imm(left, 0, new_w, h)
    } else {
        let new_h = (w as f64 / target_ratio) as u32;
        let top = h.saturating_sub(new_h) / 2;
        img.crop_imm(0, top, w, new_h)
    };

    cropped.resize_exact(target_w, target_h, imageops::FilterType::Lanczos3)
}

#[must_use]
pub fn extract_dominant_hue(img: &image::DynamicImage) -> (u16, u16) {
    let small = img.resize_exact(32, 32, imageops::FilterType::Nearest);
    let rgb = small.to_rgb8();

    let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
    let mut count = 0u64;

    for p in rgb.pixels() {
        r_sum += p[0] as u64;
        g_sum += p[1] as u64;
        b_sum += p[2] as u64;
        count += 1;
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
    let hue = (h as i64).rem_euclid(360) as u16;

    (hue, sat)
}

pub fn image_to_data_uri(img: &image::DynamicImage, quality: u8) -> Result<String, RenderError> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    rgb.write_with_encoder(encoder).map_err(|e| {
        RenderError::Encode(log_fmt!("render.err_jpeg_encode", error = e).to_string())
    })?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

/// 用户基本信息上下文，供 profile / 成绩卡片渲染共用。
///
/// 包含用户名、模式、pp、各级排名及其变化量、头像 URL 等展示字段。
pub struct UserContext<'a> {
    pub username: &'a str,
    pub mode: osubot_types::GameMode,
    pub user_pp: f64,
    pub user_global_rank: Option<i64>,
    pub user_country_rank: Option<i64>,
    pub country_code: &'a str,
    pub avatar_url: &'a str,
    pub pp_change: Option<f64>,
    pub global_rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
}

/// 单条成绩卡片渲染参数。
///
/// 聚合用户上下文、成绩数据、谱面状态及可选的有效难度属性（AR/OD/CS/HP）、
/// 封面图与取消标志，传入 [`render_score_card`]。
pub struct ScoreCardParams<'a> {
    pub user: UserContext<'a>,
    pub score: &'a osubot_types::Score,
    pub play_time: &'a str,
    pub fav_count: Option<i64>,
    pub play_count: Option<i64>,
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
        user,
        score,
        play_time,
        fav_count,
        play_count,
        ranked_status,
        ur_value,
        ar_eff,
        od_eff,
        cs_eff,
        hp_eff,
        cover_image: preloaded_cover_image,
        cancel_flag: external_cancel,
    } = params;
    let UserContext {
        username,
        mode,
        user_pp,
        user_global_rank,
        user_country_rank,
        country_code,
        avatar_url,
        pp_change,
        global_rank_change,
        country_rank_change,
    } = user;
    let client = cache::http_client();

    tracing::debug!("{}", log_fmt!("render.download_avatar", url = avatar_url));
    let avatar_bytes = if !avatar_url.is_empty() {
        match cache::fetch_and_cache(avatar_url, client, true).await {
            Ok((bytes, _, _)) => bytes,
            Err(e) => {
                tracing::warn!("{}", log_fmt!("render.avatar_download_failed", error = &e));
                vec![]
            }
        }
    } else {
        vec![]
    };
    tracing::debug!(
        "{}",
        log_fmt!("render.avatar_downloaded", bytes = avatar_bytes.len())
    );

    tracing::debug!("{}", log_fmt!("render.preprocess_cover"));
    let (bg_uri, thumb_uri, hue, sat) = if let Some(ref img) = preloaded_cover_image {
        let img_clone = img.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<_, RenderError> {
            let bg = crop_and_resize(&img_clone, 1920, 1080);
            let bg_uri = image_to_data_uri(&bg, 85)?;

            let thumb = crop_and_resize(&img_clone, 384, 216);
            let thumb_uri = image_to_data_uri(&thumb, 90)?;

            let (h, s) = extract_dominant_hue(&img_clone);

            Ok((bg_uri, thumb_uri, h, s))
        })
        .await
        .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))?;
        result?
    } else {
        (String::new(), String::new(), 255, 30)
    };

    let avatar_uri = if !avatar_bytes.is_empty() {
        let avatar_bytes = avatar_bytes.clone();
        let resized_uri = tokio::task::spawn_blocking(move || -> Result<String, RenderError> {
            let img = image::load_from_memory(&avatar_bytes).map_err(|e| {
                RenderError::Render(log_fmt!("render.err_avatar_decode", error = e).to_string())
            })?;
            let resized = img.resize_exact(116, 116, imageops::FilterType::Lanczos3);
            image_to_data_uri(&resized, 85)
        })
        .await
        .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))?;
        resized_uri?
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
    tracing::debug!("{}", log_fmt!("render.html_generated"));

    let (pixels, w, h) = run_render(html, 1920, 1080, 60, external_cancel).await?;
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
    let client = cache::http_client();

    tracing::debug!("{}", log_fmt!("render.download_avatar", url = avatar_url));
    let avatar_data_uri = if !avatar_url.is_empty() {
        match cache::fetch_and_cache(avatar_url, client, true).await {
            Ok((bytes, _, _)) => {
                let resized_uri =
                    tokio::task::spawn_blocking(move || -> Result<String, RenderError> {
                        let img = image::load_from_memory(&bytes).map_err(|e| {
                            RenderError::Render(
                                log_fmt!("render.err_avatar_decode", error = e).to_string(),
                            )
                        })?;
                        let resized = img.resize_exact(200, 200, imageops::FilterType::Lanczos3);
                        image_to_data_uri(&resized, 85)
                    })
                    .await
                    .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))?;
                resized_uri?
            }
            Err(e) => {
                tracing::warn!("{}", log_fmt!("render.avatar_download_failed", error = &e));
                String::new()
            }
        }
    } else {
        tracing::debug!(
            "{}",
            log_fmt!("render.profile_avatar_url_empty", user = username)
        );
        String::new()
    };
    tracing::debug!(
        "{}",
        log_fmt!("render.avatar_downloaded", bytes = avatar_data_uri.len())
    );

    let wrapped_html =
        style::build_profile_html(html, profile_hue, &avatar_data_uri, username).await;
    let (mut pixels, mut w, mut h) = run_render(wrapped_html, width, height, 60, None).await?;

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

/// 多条成绩列表卡片渲染参数。
///
/// 聚合用户上下文、成绩列表、标签/计数文本、每条成绩的封面图及头部大图 URL，
/// 传入 [`render_score_list_card`]。
pub struct ScoreListCardParams<'a> {
    pub user: UserContext<'a>,
    pub scores: &'a [osubot_types::Score],
    pub label: &'a str,
    pub count_text: &'a str,
    pub cover_images: Vec<Option<image::DynamicImage>>,
    pub hero_cover_url: &'a str,
    pub original_indices: &'a [usize],
}

pub async fn render_score_list_card(
    params: ScoreListCardParams<'_>,
) -> Result<Vec<u8>, RenderError> {
    let ScoreListCardParams {
        user,
        scores,
        label,
        count_text,
        cover_images,
        hero_cover_url,
        original_indices,
    } = params;
    let UserContext {
        username,
        mode,
        user_pp,
        user_global_rank,
        user_country_rank,
        country_code,
        avatar_url,
        pp_change,
        global_rank_change,
        country_rank_change,
    } = user;

    // Download avatar and hero banner in parallel
    let client = cache::http_client();
    let (avatar_uri, hero_bg_uri) = {
        let (avatar_result, hero_result) = tokio::join!(
            async {
                if avatar_url.is_empty() {
                    return Ok(String::new());
                }
                let (bytes, _, _) = cache::fetch_and_cache(avatar_url, client, true)
                    .await
                    .map_err(|e| {
                        RenderError::Render(
                            log_fmt!("render.err_avatar_fetch", error = e).to_string(),
                        )
                    })?;
                let resized_uri =
                    tokio::task::spawn_blocking(move || -> Result<String, RenderError> {
                        let img = image::load_from_memory(&bytes).map_err(|e| {
                            RenderError::Render(
                                log_fmt!("render.err_avatar_decode", error = e).to_string(),
                            )
                        })?;
                        let resized = img.resize_exact(120, 120, imageops::FilterType::Lanczos3);
                        image_to_data_uri(&resized, 85)
                    })
                    .await
                    .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))?;
                resized_uri
            },
            async {
                if hero_cover_url.is_empty() {
                    return Ok(String::new());
                }
                let (bytes, _, _) = cache::fetch_and_cache(hero_cover_url, client, false)
                    .await
                    .map_err(|e| {
                        RenderError::Render(
                            log_fmt!("render.err_hero_fetch", error = e).to_string(),
                        )
                    })?;
                let cropped_uri =
                    tokio::task::spawn_blocking(move || -> Result<String, RenderError> {
                        let img = image::load_from_memory(&bytes).map_err(|_| {
                            RenderError::Render(log_fmt!("render.err_hero_decode").to_string())
                        })?;
                        let cropped = crop_and_resize(&img, 1920, 480);
                        Ok(match image_to_data_uri(&cropped, 80) {
                            Ok(uri) => uri,
                            Err(e) => {
                                tracing::warn!(error = %e, "JPEG encoding failed for hero image");
                                String::new()
                            }
                        })
                    })
                    .await
                    .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))?;
                cropped_uri
            },
        );
        (avatar_result?, hero_result?)
    };

    // Process cover thumbnails in parallel
    let cover_futures: Vec<_> = cover_images
        .iter()
        .filter_map(|opt| {
            let img = opt.as_ref()?.clone();
            Some(tokio::task::spawn_blocking(move || -> String {
                let thumb = crop_and_resize(&img, 465, 165);
                match image_to_data_uri(&thumb, 70) {
                    Ok(uri) => uri,
                    Err(e) => {
                        tracing::warn!(error = %e, "JPEG encoding failed for cover image");
                        String::new()
                    }
                }
            }))
        })
        .collect();
    let mut cover_uris: Vec<String> = Vec::with_capacity(cover_images.len());
    for future in cover_futures {
        match future.await {
            Ok(uri) => cover_uris.push(uri),
            Err(e) => {
                tracing::warn!(error = %e, "spawn_blocking failed for cover thumbnail");
                cover_uris.push(String::new());
            }
        }
    }

    // Build card data
    let cards: Vec<score_list_style::ScoreListCardData> = scores
        .iter()
        .enumerate()
        .map(|(i, score)| {
            let cover_uri = cover_uris.get(i).cloned().unwrap_or_default();
            score_list_style::ScoreListCardData::from_score(score, cover_uri)
        })
        .collect();

    let html_params = score_list_style::ScoreListHtmlParams {
        cards: &cards,
        username,
        mode,
        label,
        count_text,
        avatar_data_uri: &avatar_uri,
        hero_bg_data_uri: &hero_bg_uri,
        user_pp,
        user_global_rank,
        user_country_rank,
        country_code,
        pp_change,
        global_rank_change,
        country_rank_change,
        original_indices,
    };
    let html = score_list_style::wrap_score_list_html(&html_params);

    // Estimate height: 480px hero (matches the 1920x480 banner image; .hero has
    // min-height: 480px so background-size: cover is effectively 100% 100%)
    // + 36px score-list padding + ceil(N/4) rows of 330px cards.
    // Card height ≈ 165px cover + 165px body (padding + title + sub + mods + rows).
    let rows = (scores.len() as u32).div_ceil(4);
    let estimated_height = 480 + 36 + rows * 330;

    let (pixels, w, h) = run_render(
        html,
        1920,
        estimated_height,
        SCORE_LIST_RENDER_TIMEOUT_SECS,
        None,
    )
    .await?;
    let jpeg = encode::encode_jpeg(pixels, w, h, 90).await?;
    Ok(jpeg)
}

pub struct MatchResultPlayerParams {
    pub placement: usize,
    pub username: String,
    pub avatar_url: Option<String>,
    pub avatar_image: Option<image::DynamicImage>,
    pub team: Option<String>,
    pub score: u64,
    pub accuracy: f64,
    pub max_combo: u32,
    pub mods: Vec<String>,
    pub rank: String,
    pub passed: bool,
}

pub struct MatchResultParams {
    pub match_id: u64,
    pub match_name: String,
    pub event_label: String,
    pub played_at: String,
    pub beatmap_id: u64,
    pub beatmap_artist: String,
    pub beatmap_title: String,
    pub beatmap_version: String,
    pub beatmap_mapper: String,
    pub beatmap_mode: String,
    pub star_rating: Option<f64>,
    pub beatmap_bpm: Option<f64>,
    pub beatmap_length_seconds: Option<u32>,
    pub beatmap_max_combo: Option<u32>,
    pub beatmap_ar: Option<f64>,
    pub beatmap_od: Option<f64>,
    pub beatmap_cs: Option<f64>,
    pub beatmap_hp: Option<f64>,
    pub cover_image: Option<image::DynamicImage>,
    pub is_started: bool,
    pub selected_mods: Vec<String>,
    pub team_type: Option<String>,
    pub scoring_type: Option<String>,
    pub team_results: Vec<MatchTeamResultParams>,
    pub players: Vec<MatchResultPlayerParams>,
}

pub struct MatchTeamResultParams {
    pub team: String,
    pub score: u64,
    pub is_winner: bool,
}

pub async fn render_match_result_card(params: MatchResultParams) -> Result<Vec<u8>, RenderError> {
    let MatchResultParams {
        match_id,
        match_name,
        event_label,
        played_at,
        beatmap_id,
        beatmap_artist,
        beatmap_title,
        beatmap_version,
        beatmap_mapper,
        beatmap_mode,
        star_rating,
        beatmap_bpm,
        beatmap_length_seconds,
        beatmap_max_combo,
        beatmap_ar,
        beatmap_od,
        beatmap_cs,
        beatmap_hp,
        cover_image,
        is_started,
        selected_mods,
        team_type,
        scoring_type,
        team_results,
        players,
    } = params;

    let cover_data_uri = if let Some(img) = cover_image {
        let uri = tokio::task::spawn_blocking(move || -> Result<String, RenderError> {
            let bg = crop_and_resize(&img, 1920, 1080);
            image_to_data_uri(&bg, 85)
        })
        .await
        .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))?;
        uri?
    } else {
        String::new()
    };

    let players: Vec<match_style::MatchPlayerRowData> = tokio::task::spawn_blocking(
        move || -> Result<Vec<match_style::MatchPlayerRowData>, RenderError> {
            players
                .into_iter()
                .map(|p| {
                    let avatar_data_uri = p
                        .avatar_image
                        .map(|img| {
                            let avatar = img.resize_exact(96, 96, imageops::FilterType::Lanczos3);
                            image_to_data_uri(&avatar, 85)
                        })
                        .transpose()?;
                    Ok(match_style::MatchPlayerRowData {
                        placement: p.placement,
                        username: p.username,
                        avatar_data_uri: avatar_data_uri.unwrap_or_default(),
                        team: p.team,
                        score: p.score,
                        accuracy: p.accuracy,
                        max_combo: p.max_combo,
                        mods: p.mods,
                        rank: p.rank,
                        passed: p.passed,
                    })
                })
                .collect()
        },
    )
    .await
    .map_err(|e| RenderError::Render(format!("spawn_blocking failed: {e}")))??;

    let data = match_style::MatchResultCardData {
        match_id,
        match_name,
        event_label,
        played_at,
        beatmap_id,
        beatmap_artist,
        beatmap_title,
        beatmap_version,
        beatmap_mapper,
        beatmap_mode,
        star_rating,
        beatmap_bpm,
        beatmap_length_seconds,
        beatmap_max_combo,
        beatmap_ar,
        beatmap_od,
        beatmap_cs,
        beatmap_hp,
        cover_data_uri,
        is_started,
        selected_mods,
        team_type,
        scoring_type,
        team_results: team_results
            .into_iter()
            .map(|team| match_style::MatchTeamResultData {
                team: team.team,
                score: team.score,
                is_winner: team.is_winner,
            })
            .collect(),
        players,
    };

    let html = match_style::wrap_match_result_html(&data);
    let (pixels, w, h) = run_render(html, 1920, 1080, 60, None).await?;
    encode::encode_jpeg(pixels, w, h, 90).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn render_match_result_card_produces_jpeg() {
        let params = MatchResultParams {
            match_id: 12345678,
            match_name: "Test Match".to_string(),
            event_label: "Game finished".to_string(),
            played_at: "2026/06/26 20:00:00".to_string(),
            beatmap_id: 987654,
            beatmap_artist: "xi".to_string(),
            beatmap_title: "Blue Zenith".to_string(),
            beatmap_version: "FOUR DIMENSIONS".to_string(),
            beatmap_mapper: "Asphyxia".to_string(),
            beatmap_mode: "osu!".to_string(),
            star_rating: Some(7.85),
            beatmap_bpm: Some(190.0),
            beatmap_length_seconds: Some(260),
            beatmap_max_combo: Some(2429),
            beatmap_ar: Some(9.7),
            beatmap_od: Some(9.8),
            beatmap_cs: Some(4.0),
            beatmap_hp: Some(6.0),
            cover_image: None,
            is_started: false,
            selected_mods: vec!["HD".to_string()],
            team_type: Some("team-vs".to_string()),
            scoring_type: Some("score".to_string()),
            team_results: Vec::new(),
            players: vec![
                MatchResultPlayerParams {
                    placement: 1,
                    username: "Alice".to_string(),
                    avatar_url: None,
                    avatar_image: None,
                    team: Some("Red".to_string()),
                    score: 1_234_567,
                    accuracy: 0.9876,
                    max_combo: 1234,
                    mods: vec!["HD".to_string(), "HR".to_string()],
                    rank: "A".to_string(),
                    passed: true,
                },
                MatchResultPlayerParams {
                    placement: 2,
                    username: "Bob".to_string(),
                    avatar_url: None,
                    avatar_image: None,
                    team: Some("Blue".to_string()),
                    score: 987_654,
                    accuracy: 0.9654,
                    max_combo: 876,
                    mods: Vec::new(),
                    rank: "F".to_string(),
                    passed: false,
                },
            ],
        };

        let jpeg = render_match_result_card(params).await.expect("render ok");
        assert!(!jpeg.is_empty());
        assert!(jpeg.len() > 1024);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn test_score_list_estimated_height_matches_1920_css() {
        let source = include_str!("lib.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(
            production.contains("rows * 330"),
            "score-list height estimate should match 1920 CSS row height"
        );
    }

    #[tokio::test]
    async fn test_extract_panic_message_from_str_panic() {
        let err = tokio::task::spawn_blocking(|| {
            panic!("intentional test panic: something went wrong");
        })
        .await
        .unwrap_err();
        let msg = extract_panic_message(err);
        assert!(msg.contains("intentional test panic: something went wrong"));
    }

    #[tokio::test]
    async fn test_extract_panic_message_from_string_panic() {
        let s = String::from("owned string panic");
        let err = tokio::task::spawn_blocking(move || {
            panic!("{}", s);
        })
        .await
        .unwrap_err();
        let msg = extract_panic_message(err);
        assert!(msg.contains("owned string panic"));
    }

    #[tokio::test]
    async fn test_extract_panic_message_cancelled() {
        let handle = tokio::task::spawn_blocking(|| {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        });
        handle.abort_handle().abort();
        let err = handle.await.unwrap_err();
        let msg = extract_panic_message(err);
        assert!(msg.contains("cancelled") || msg.contains("Cancelled") || msg.contains("task"));
    }

    #[test]
    fn test_render_error_timeout_variant() {
        let err = RenderError::Timeout;
        assert_eq!(err.to_string(), "Render timeout");
    }

    #[test]
    fn test_render_error_cancelled_variant() {
        let err = RenderError::Cancelled;
        assert_eq!(err.to_string(), "Render cancelled");
    }

    #[test]
    fn test_render_error_panicked_variant() {
        let err = RenderError::Panicked("something broke".to_string());
        assert_eq!(err.to_string(), "Render panicked: something broke");
    }

    #[test]
    fn test_render_error_html_render_variant() {
        let err = RenderError::HtmlRender("layout error".to_string());
        assert_eq!(err.to_string(), "HTML render error: layout error");
    }

    #[test]
    fn test_render_error_render_variant() {
        let err = RenderError::Render("test error".to_string());
        assert_eq!(err.to_string(), "Render failed: test error");
    }

    #[test]
    fn test_render_error_encode_variant() {
        let err = RenderError::Encode("encode error".to_string());
        assert_eq!(err.to_string(), "Encode failed: encode error");
    }

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
