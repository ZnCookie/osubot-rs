//! 手动测试：dump 喂给 !profile 渲染器的最终 HTML。
//!
//! 不参与 `cargo test` 默认运行（`#[ignore]`）。手动执行：
//!
//! ```text
//! USERNAME=foo OSU_CLIENT_ID=... OSU_CLIENT_SECRET=... \
//!     cargo test --package osubot-render --test dump_profile_html \
//!     -- --ignored --nocapture
//! ```
//!
//! 产物：`target/dump-profile-html/<sanitized-username>.html`
//! stdout 一行摘要：`wrote N bytes to <path> for user "..." (osu_id=..., hue=...)`

use osubot_core::api::{fetch_user_profile, fetch_user_stats_by_username, OauthTokenCache};
use osubot_core::RateLimiter;
use osubot_render::style::build_profile_html;
use osubot_types::GameMode;
use std::path::PathBuf;

#[tokio::test]
#[ignore = "manual: hits osu! API; needs USERNAME, OSU_CLIENT_ID, OSU_CLIENT_SECRET"]
async fn dump_profile_html_for_username() {
    let username = std::env::var("USERNAME").expect("USERNAME env var not set");
    let client_id = std::env::var("OSU_CLIENT_ID").expect("OSU_CLIENT_ID env var not set");
    let client_secret =
        std::env::var("OSU_CLIENT_SECRET").expect("OSU_CLIENT_SECRET env var not set");

    osubot_render::cache::ensure_cache_dir().await;

    let oauth = OauthTokenCache::new(client_id, client_secret);
    let rate_limiter = RateLimiter::new();

    let stats = fetch_user_stats_by_username(&rate_limiter, &oauth, &username, GameMode::Osu)
        .await
        .expect("failed to resolve username via osu! API");
    let user_id = stats.user_id;

    let profile = fetch_user_profile(&rate_limiter, &oauth, user_id, GameMode::Osu)
        .await
        .expect("failed to fetch user profile");
    // 预取头像并转 data URI（与 render_profile_card 同款逻辑）
    let avatar_data_uri = if !profile.avatar_url.is_empty() {
        match osubot_render::cache::fetch_and_cache(
            &profile.avatar_url,
            osubot_render::cache::http_client(),
            true,
        )
        .await
        {
            Ok((bytes, _, _)) => {
                let uri = tokio::task::spawn_blocking(move || {
                    let img = image::load_from_memory(&bytes)
                        .map_err(|e| format!("avatar decode: {e}"))?;
                    let resized = img.resize_exact(200, 200, image::imageops::FilterType::Lanczos3);
                    osubot_render::image_to_data_uri(&resized, 85)
                        .map_err(|e| format!("data uri: {e}"))
                })
                .await
                .map_err(|e| format!("spawn_blocking: {e}"))
                .and_then(|r| r)
                .unwrap_or_default();
                uri
            }
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    let html = build_profile_html(
        &profile.html,
        profile.profile_hue,
        &avatar_data_uri,
        &profile.username,
    )
    .await;

    let out_dir = PathBuf::from("target/dump-profile-html");
    std::fs::create_dir_all(&out_dir).expect("failed to create dump dir");
    let out_path = out_dir.join(format!("{}.html", sanitize(&profile.username)));
    std::fs::write(&out_path, &html).expect("failed to write HTML");

    println!(
        "wrote {} bytes to {} for user {:?} (osu_id={}, hue={})",
        html.len(),
        out_path.display(),
        profile.username,
        user_id,
        profile.profile_hue,
    );
}

/// 把 osu! 用户名清洗成安全文件名（仅保留 `a-zA-Z0-9_-`，其它字符替换为 `_`）。
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
