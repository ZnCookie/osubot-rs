//! 分数查询与渲染。
//!
//! 行号注释约定：`L<n>-` / `L<a>-<b>` 标注紧随其后的代码块对应的行号范围，
//! 便于定位与维护。`FnOnce` 闭包传入 `RequestDedup::run_or_wait` 时，
//! 在闭包内重新 clone `ctx.rate_limiter` / `ctx.oauth` 以满足 `'static`。

use crate::score_filter::score_matches_filters;
use crate::BotContext;
use futures_util::future::join_all;
use osubot_core::apply_mod_adjustment_to_stats;
use osubot_core::enrich_score_with_pp;
use osubot_core::{
    api, log_fmt,
    response::{format_score, format_scores},
    strings::user_str,
    types::{format_play_datetime, Command, GameMode, Score, UserStats},
};
use osubot_render::cache as render_cache;
use osubot_render::SCORE_LIST_RENDER_TIMEOUT_SECS;
use osubot_render::{render_score_card, render_score_list_card};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::warn;

use osubot_core::OauthTokenCache;
use osubot_core::RateLimiter;

use crate::api_error_msg;
use crate::onebot::{send_group_msg_with_image, QQMessage};
use crate::{
    audio_score_dedup, beatmap_scores_dedup, beatmapset_dedup, best_scores_dedup,
    preview_score_dedup, score_by_id_dedup, score_by_id_err_msg, score_dedup,
    today_best_scores_dedup, DedupApiError, SCORE_API_FETCH_LIMIT,
};

mod audio;
mod preview;
mod render;

use render::{render_and_send_score_list, render_and_send_single_score, SingleScoreRenderParams};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderOutput {
    SingleScoreCard,
    ScoreListCard,
    Audio,
    BeatmapPreview,
}

pub(crate) trait FetchFn: Send + Sync {
    fn call(
        &self,
        rate_limiter: &Arc<RateLimiter>,
        oauth: &Arc<OauthTokenCache>,
        user_id: i64,
        mode: GameMode,
        limit: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Score>, String>> + Send + '_>>;
}

/// 生成 score fetch 闭包中的错误处理函数。
fn score_fetch_error_handler(qq: i64) -> impl Fn(api::ApiError) -> String + Clone {
    move |e: api::ApiError| {
        if !matches!(
            e,
            api::ApiError::NotFound
                | api::ApiError::RateLimitedWithRetryAfter(_)
                | api::ApiError::ClientRateLimited
        ) {
            tracing::error!(
                user_id = qq,
                error = ?e,
                "{}",
                log_fmt!("main.score_query_error_details")
            );
        }
        api_error_msg(qq, &e)
    }
}

fn summary_or_single(is_summary: bool) -> RenderOutput {
    if is_summary {
        RenderOutput::ScoreListCard
    } else {
        RenderOutput::SingleScoreCard
    }
}

/// 缓存 user_id 到 storage，失败时 warn 日志。
async fn cache_user_id(ctx: &BotContext, stats: &UserStats) {
    if let Err(e) = ctx
        .storage
        .set_user_id(&stats.username, stats.user_id)
        .await
    {
        tracing::warn!(
            username = %stats.username,
            user_id = stats.user_id,
            error = %e,
            "{}",
            log_fmt!("main.cache_user_id_failed")
        );
    }
}

/// 发送"无匹配"错误消息。
async fn send_no_match(resp_tx: &mpsc::Sender<String>, qq: i64, noun_key: &'static str) {
    let _ = resp_tx
        .send(
            user_str("query.no_match")
                .replace("{qq}", &qq.to_string())
                .replace("{name}", user_str(noun_key)),
        )
        .await;
}

/// 发送"索引超出范围"错误消息。
async fn send_index_out_of_range(
    resp_tx: &mpsc::Sender<String>,
    qq: i64,
    noun_key: &'static str,
    pos: u32,
    total: usize,
) {
    let _ = resp_tx
        .send(
            user_str("query.index_out_of_range")
                .replace("{qq}", &qq.to_string())
                .replace("{pos}", &pos.to_string())
                .replace("{name}", user_str(noun_key))
                .replace("{total}", &total.to_string()),
        )
        .await;
}

fn make_fetch<F, Fut>(f: F) -> Box<dyn FetchFn>
where
    F: Fn(Arc<RateLimiter>, Arc<OauthTokenCache>, i64, GameMode, u32) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: Future<Output = Result<Vec<Score>, String>> + Send + 'static,
{
    struct Impl<F>(F);
    impl<F, Fut> FetchFn for Impl<F>
    where
        F: Fn(Arc<RateLimiter>, Arc<OauthTokenCache>, i64, GameMode, u32) -> Fut
            + Send
            + Sync
            + 'static,
        Fut: Future<Output = Result<Vec<Score>, String>> + Send + 'static,
    {
        fn call(
            &self,
            rl: &Arc<RateLimiter>,
            oa: &Arc<OauthTokenCache>,
            uid: i64,
            mode: GameMode,
            limit: u32,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Score>, String>> + Send + '_>> {
            Box::pin((self.0)(rl.clone(), oa.clone(), uid, mode, limit))
        }
    }
    Box::new(Impl(f))
}

pub(crate) struct ScoreQuerySpec {
    pub(crate) fetch: Box<dyn FetchFn>,
    pub(crate) post_process: Option<fn(&mut Vec<Score>)>,
    pub(crate) render: RenderOutput,
    pub(crate) label_key: &'static str,
    pub(crate) noun_key: &'static str,
    pub(crate) empty_msg_key: &'static str,
    pub(crate) truncate_bare_list: bool,
    pub(crate) single_needs_backfill: bool,
    pub(crate) api_fetch_limit: Option<u32>,
    pub(crate) preview_mods: Option<Vec<String>>,
    pub(crate) preview_gif: bool,
    pub(crate) preview_times: Option<Vec<i64>>,
}

struct PipelineParams<'a> {
    username: Option<&'a str>,
    qq: Option<i64>,
    beatmap_id: Option<i64>,
    limit: u32,
    limit_end: Option<u32>,
    is_summary: bool,
    filters: Option<&'a [String]>,
}

/// 发送错误消息到响应通道。
async fn resolve_score_user(
    ctx: &BotContext,
    msg: &QQMessage,
    username: &Option<String>,
    qq: &Option<i64>,
    mode: GameMode,
    resp_tx: &mpsc::Sender<String>,
) -> Option<(i64, String, UserStats)> {
    tracing::trace!("{}", log_fmt!("main.resolve_score_user_start"));

    if let Some(ref name) = username {
        tracing::trace!(
            "{}",
            log_fmt!("main.resolve_score_user_lookup", username = name)
        );
        match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, name, mode).await {
            Ok(stats) => {
                cache_user_id(ctx, &stats).await;
                Some((stats.user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, username = %name, "{}", log_fmt!("main.resolve_score_user_api_failed"));
                let err_msg = match e {
                    api::ApiError::NotFound => user_str("error.not_found_named")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{name}", name),
                    other => api_error_msg(msg.user_id, &other),
                };
                let _ = resp_tx.send(err_msg).await;
                None
            }
        }
    } else {
        let (user_id, _stored_name, error_msg) = if let Some(mentioned_qq) = qq {
            match ctx.resolve_binding(*mentioned_qq).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some(user_str("bind.user_not_bound").replace("{qq}", &msg.user_id.to_string())),
                ),
            }
        } else {
            match ctx.resolve_binding(msg.user_id).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string())),
                ),
            }
        };
        if let Some(err) = error_msg {
            let _ = resp_tx.send(err).await;
            return None;
        }
        tracing::info!(
            "{}",
            log_fmt!("main.resolve_score_user_fetch_stats", user_id = user_id)
        );
        match api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, user_id, mode).await {
            Ok(stats) => {
                cache_user_id(ctx, &stats).await;
                Some((user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id, "{}", log_fmt!("main.resolve_score_user_lookup_bound_failed"));
                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                None
            }
        }
    }
}

/// 通过 score_id 直接获取成绩，发送错误消息后返回 None。
async fn fetch_score_by_id(
    ctx: &BotContext,
    score_id: u64,
    user_id: i64,
    resp_tx: &mpsc::Sender<String>,
) -> Option<Score> {
    let rl = ctx.rate_limiter.clone();
    let oauth = ctx.oauth.clone();
    let result = score_by_id_dedup()
        .run_or_wait(score_id as i64, move || {
            let rl = rl.clone();
            let oauth = oauth.clone();
            async move {
                api::get_score_by_id(&rl, &oauth, score_id)
                    .await
                    .map_err(|e| {
                        if !matches!(e, api::ApiError::NotFound) {
                            tracing::warn!(
                                error = ?e,
                                "{}",
                                log_fmt!("main.get_score_by_id_failed")
                            );
                        }
                        DedupApiError::from_api_error(&e)
                    })
            }
        })
        .await;
    match result {
        Ok(s) => Some(s),
        Err(e) => {
            let _ = resp_tx.send(score_by_id_err_msg(user_id, &e)).await;
            None
        }
    }
}

/// 处理 score_id 直达后的成绩卡片渲染（获取用户 stats + 渲染单条成绩）。
async fn handle_score_id_render(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    score: &Score,
    mode: GameMode,
    label: &'static str,
) {
    ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
    let user_id = score.user.user_id.unwrap_or(0);
    if user_id == 0 {
        let _ = resp_tx
            .send(user_str("query.user_info_failed").replace("{qq}", &msg.user_id.to_string()))
            .await;
        return;
    }
    let user_stats = match api::fetch_user_stats_by_user_id(
        &ctx.rate_limiter,
        &ctx.oauth,
        user_id,
        mode,
    )
    .await
    {
        Ok(stats) => {
            cache_user_id(ctx, &stats).await;
            ctx.scheduler.trigger_update(user_id, mode).await;
            stats
        }
        Err(e) => {
            if !matches!(e, api::ApiError::NotFound) {
                tracing::warn!(
                    user_id = user_id,
                    error = ?e,
                    "{}",
                    log_fmt!("main.fetch_stats_score_id_failed")
                );
            }
            let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
            return;
        }
    };
    render_and_send_single_score(SingleScoreRenderParams {
        ctx,
        msg,
        resp_tx,
        score,
        mode,
        user_stats: &user_stats,
        // TODO: 对于 !b <score_id> 等场景，可考虑额外 API 调用获取排名
        position: None,
        label: user_str(label),
    })
    .await;
}

/// 尝试 score_id 直达：如果 score_id 存在，获取成绩并渲染，返回 true。
/// 否则返回 false，调用方应继续走正常流程。
async fn try_score_id_early_return(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    score_id: &Option<u64>,
    mode: GameMode,
    label: &'static str,
) -> bool {
    let Some(sid) = score_id else {
        return false;
    };
    let Some(score) = fetch_score_by_id(ctx, *sid, msg.user_id, resp_tx).await else {
        return true; // fetch 失败，已发送错误消息
    };
    handle_score_id_render(ctx, msg, resp_tx, &score, mode, label).await;
    true
}

pub(crate) async fn handle_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    let qq_for_err = msg.user_id;
    let (spec, params) = match cmd {
        Command::Pass {
            username,
            qq,
            limit,
            limit_end,
            is_summary,
            beatmap_id,
            score_id,
            filters,
            ..
        } => {
            if try_score_id_early_return(ctx, msg, resp_tx, score_id, mode, "fmt.recent_pass").await
            {
                return;
            }

            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        score_dedup()
                            .run_or_wait((uid, true, l, m), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_recent(&rl, &oa, uid, m, false, l, false)
                                        .await
                                        .map_err(score_fetch_error_handler(qq_for_err))
                                        .map(Arc::new)
                                }
                            })
                            .await
                            .map(|arc| (*arc).clone())
                    }),
                    post_process: None,
                    render: summary_or_single(*is_summary),
                    label_key: "fmt.recent_pass",
                    noun_key: "query.noun_replay",
                    empty_msg_key: "query.no_records_pass",
                    truncate_bare_list: false,
                    single_needs_backfill: true,
                    api_fetch_limit: None,
                    preview_mods: None,
                    preview_gif: false,
                    preview_times: None,
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    beatmap_id: beatmap_id.map(|b| b as i64),
                    limit: *limit,
                    limit_end: *limit_end,
                    is_summary: *is_summary,
                    filters: filters.as_deref(),
                },
            )
        }
        Command::Recent {
            username,
            qq,
            limit,
            limit_end,
            is_summary,
            beatmap_id,
            score_id,
            filters,
            ..
        } => {
            if try_score_id_early_return(ctx, msg, resp_tx, score_id, mode, "fmt.recent_play").await
            {
                return;
            }

            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        score_dedup()
                            .run_or_wait((uid, false, l, m), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_recent(&rl, &oa, uid, m, true, l, false)
                                        .await
                                        .map_err(score_fetch_error_handler(qq_for_err))
                                        .map(Arc::new)
                                }
                            })
                            .await
                            .map(|arc| (*arc).clone())
                    }),
                    post_process: None,
                    render: summary_or_single(*is_summary),
                    label_key: "fmt.recent_play",
                    noun_key: "query.noun_replay",
                    empty_msg_key: "query.no_records",
                    truncate_bare_list: false,
                    single_needs_backfill: true,
                    api_fetch_limit: None,
                    preview_mods: None,
                    preview_gif: false,
                    preview_times: None,
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    beatmap_id: beatmap_id.map(|b| b as i64),
                    limit: *limit,
                    limit_end: *limit_end,
                    is_summary: *is_summary,
                    filters: filters.as_deref(),
                },
            )
        }
        Command::Best {
            username,
            qq,
            beatmap_id,
            score_id,
            limit,
            limit_end,
            is_summary,
            filters,
            ..
        } => {
            if try_score_id_early_return(ctx, msg, resp_tx, score_id, mode, "fmt.best_score").await
            {
                return;
            }

            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        best_scores_dedup()
                            .run_or_wait((uid, m, l), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_best(&rl, &oa, uid, m, l)
                                        .await
                                        .map_err(score_fetch_error_handler(qq_for_err))
                                }
                            })
                            .await
                    }),
                    post_process: None,
                    render: summary_or_single(*is_summary),
                    label_key: "fmt.best_score",
                    noun_key: "query.noun_best",
                    empty_msg_key: "query.no_records_best",
                    truncate_bare_list: false,
                    single_needs_backfill: true,
                    api_fetch_limit: None,
                    preview_mods: None,
                    preview_gif: false,
                    preview_times: None,
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    beatmap_id: beatmap_id.map(|b| b as i64),
                    limit: *limit,
                    limit_end: *limit_end,
                    is_summary: *is_summary,
                    filters: filters.as_deref(),
                },
            )
        }
        Command::TodayBest {
            username,
            qq,
            beatmap_id,
            score_id,
            limit,
            limit_end,
            is_summary,
            filters,
            ..
        } => {
            if try_score_id_early_return(ctx, msg, resp_tx, score_id, mode, "fmt.today_best").await
            {
                return;
            }

            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        today_best_scores_dedup()
                            .run_or_wait((uid, m, l), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_best(&rl, &oa, uid, m, l)
                                        .await
                                        .map_err(score_fetch_error_handler(qq_for_err))
                                }
                            })
                            .await
                    }),
                    post_process: Some(|scores| {
                        let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
                        scores.retain(|s| {
                            chrono::DateTime::parse_from_rfc3339(&s.created_at)
                                .ok()
                                .is_some_and(|dt| dt.naive_utc() > cutoff.naive_utc())
                        });
                    }),
                    render: summary_or_single(*is_summary),
                    label_key: "fmt.today_best",
                    noun_key: "query.noun_today_best",
                    empty_msg_key: "query.no_records_today_best",
                    truncate_bare_list: true,
                    single_needs_backfill: true,
                    api_fetch_limit: Some(SCORE_API_FETCH_LIMIT),
                    preview_mods: None,
                    preview_gif: false,
                    preview_times: None,
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    beatmap_id: beatmap_id.map(|b| b as i64),
                    limit: *limit,
                    limit_end: *limit_end,
                    is_summary: *is_summary,
                    filters: filters.as_deref(),
                },
            )
        }
        Command::BeatmapAudio {
            username,
            qq,
            limit,
            filters,
            score_id,
            beatmap_id,
            explicit_position,
            mode: cmd_mode,
            ..
        } => {
            // score_id 直达：获取成绩后播放其谱面音频，无需绑定。
            if let Some(sid) = score_id {
                let score = match fetch_score_by_id(ctx, *sid, msg.user_id, resp_tx).await {
                    Some(s) => s,
                    None => return,
                };
                ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
                audio::render_audio(ctx, msg, resp_tx, &score, mode).await;
                return;
            }

            // beatmap_id 直达：解析 beatmapset_id 后播放音频，无需绑定。
            if let Some(bid) = beatmap_id {
                let beatmapset_id = match fetch_beatmapset_id_dedup(ctx, *bid as i64).await {
                    Ok(id) => id,
                    Err(e) => {
                        let _ = resp_tx.send(e.to_user_msg(msg.user_id)).await;
                        return;
                    }
                };
                ctx.last_beatmap.set(msg.group_id, *bid);
                audio::render_audio_by_beatmapset_id(ctx, msg, resp_tx, beatmapset_id).await;
                return;
            }

            // last_beatmap 缓存兜底：无参时直接用缓存 beatmap_id 转音频。
            let has_target = username.is_some()
                || qq.is_some()
                || filters.as_ref().is_some_and(|f| !f.is_empty())
                || *explicit_position
                || cmd_mode.is_some();
            if !has_target {
                if let Some(bid) = ctx.last_beatmap.get(msg.group_id) {
                    let beatmapset_id = match fetch_beatmapset_id_dedup(ctx, bid as i64).await {
                        Ok(id) => id,
                        Err(e) => {
                            let _ = resp_tx.send(e.to_user_msg(msg.user_id)).await;
                            return;
                        }
                    };
                    audio::render_audio_by_beatmapset_id(ctx, msg, resp_tx, beatmapset_id).await;
                    return;
                }
            }

            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        audio_score_dedup()
                            .run_or_wait((uid, true, l, m), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_recent(&rl, &oa, uid, m, true, l, false)
                                        .await
                                        .map_err(score_fetch_error_handler(qq_for_err))
                                        .map(Arc::new)
                                }
                            })
                            .await
                            .map(|arc| (*arc).clone())
                    }),
                    post_process: None,
                    render: RenderOutput::Audio,
                    label_key: "fmt.recent_play",
                    noun_key: "query.noun_replay",
                    empty_msg_key: "query.no_records",
                    truncate_bare_list: false,
                    single_needs_backfill: false,
                    api_fetch_limit: None,
                    preview_mods: None,
                    preview_gif: false,
                    preview_times: None,
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    limit: *limit,
                    limit_end: None,
                    is_summary: false,
                    beatmap_id: None,
                    filters: filters.as_deref(),
                },
            )
        }
        Command::BeatmapPreview {
            username,
            qq,
            limit,
            filters,
            mods,
            gif,
            times,
            score_id,
            beatmap_id,
            explicit_position,
            mode: cmd_mode,
            ..
        } => {
            // score_id 直达：获取成绩后渲染其谱面预览，无需绑定。
            if let Some(sid) = score_id {
                let score = match fetch_score_by_id(ctx, *sid, msg.user_id, resp_tx).await {
                    Some(s) => s,
                    None => return,
                };
                let resolved_bid = score.beatmap_id as u32;
                ctx.last_beatmap.set(msg.group_id, resolved_bid);
                preview::render_beatmap_preview_by_id(
                    ctx,
                    msg,
                    resp_tx,
                    mods,
                    *gif,
                    times,
                    resolved_bid,
                    mode,
                )
                .await;
                return;
            }

            // beatmap_id 直达
            if let Some(bid) = beatmap_id {
                ctx.last_beatmap.set(msg.group_id, *bid);
                preview::render_beatmap_preview_by_id(
                    ctx, msg, resp_tx, mods, *gif, times, *bid, mode,
                )
                .await;
                return;
            }

            // last_beatmap 缓存兜底
            let has_target = username.is_some()
                || qq.is_some()
                || filters.as_ref().is_some_and(|f| !f.is_empty())
                || *explicit_position
                || cmd_mode.is_some();
            if !has_target {
                if let Some(bid) = ctx.last_beatmap.get(msg.group_id) {
                    preview::render_beatmap_preview_by_id(
                        ctx, msg, resp_tx, mods, *gif, times, bid, mode,
                    )
                    .await;
                    return;
                }
            }

            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        preview_score_dedup()
                            .run_or_wait((uid, true, l, m), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_recent(&rl, &oa, uid, m, true, l, false)
                                        .await
                                        .map_err(score_fetch_error_handler(qq_for_err))
                                        .map(Arc::new)
                                }
                            })
                            .await
                            .map(|arc| (*arc).clone())
                    }),
                    post_process: None,
                    render: RenderOutput::BeatmapPreview,
                    label_key: "fmt.recent_play",
                    noun_key: "query.noun_replay",
                    empty_msg_key: "query.no_records",
                    truncate_bare_list: false,
                    single_needs_backfill: false,
                    api_fetch_limit: None,
                    preview_mods: mods.clone(),
                    preview_gif: *gif,
                    preview_times: times.clone(),
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    limit: *limit,
                    limit_end: None,
                    is_summary: false,
                    beatmap_id: None,
                    filters: filters.as_deref(),
                },
            )
        }
        Command::ScoreOnBeatmap {
            username,
            qq,
            beatmap_id,
            score_id,
            filters,
            limit,
            limit_end,
            is_all,
            ..
        } => {
            if try_score_id_early_return(ctx, msg, resp_tx, score_id, mode, "fmt.beatmap_score")
                .await
            {
                return;
            }

            let resolved_bid = match beatmap_id {
                Some(bid) => *bid as i64,
                None => match ctx.last_beatmap.get(msg.group_id) {
                    Some(bid) => bid as i64,
                    None => {
                        let _ = resp_tx
                            .send(
                                user_str("query.need_beatmap_or_cache")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                        return;
                    }
                },
            };
            ctx.last_beatmap.set(msg.group_id, resolved_bid as u32);
            (
                ScoreQuerySpec {
                    fetch: make_fetch(move |rl, oa, uid, m, l| async move {
                        beatmap_scores_dedup().run_or_wait((uid, resolved_bid, m, Some(l)), move || {
                                let rl = rl.clone();
                                let oa = oa.clone();
                                async move {
                                    api::get_user_beatmap_scores_all(&rl, &oa, resolved_bid, uid, m, Some(l), true).await.map_err(|e| {
                                        if !matches!(e, api::ApiError::NotFound) {
                                            tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                                        }
                                        match e {
                                            api::ApiError::NotFound => user_str("query.no_score_on_map").replace("{qq}", &qq_for_err.to_string()),
                                            other => api_error_msg(qq_for_err, &other),
                                        }
                                    })
                                }
                            }).await
                    }),
                    post_process: None,
                    render: if *is_all || limit_end.is_some() {
                        RenderOutput::ScoreListCard
                    } else {
                        RenderOutput::SingleScoreCard
                    },
                    label_key: "fmt.beatmap_score",
                    noun_key: "query.noun_score",
                    empty_msg_key: "query.no_score_on_map",
                    truncate_bare_list: false,
                    single_needs_backfill: true,
                    api_fetch_limit: None,
                    preview_mods: None,
                    preview_gif: false,
                    preview_times: None,
                },
                PipelineParams {
                    username: username.as_deref(),
                    qq: *qq,
                    limit: *limit,
                    limit_end: *limit_end,
                    is_summary: *is_all || limit_end.is_some(),
                    beatmap_id: Some(resolved_bid),
                    filters: filters.as_deref(),
                },
            )
        }
        _ => return,
    };

    run_score_query_pipeline(spec, params, ctx, msg, resp_tx, mode).await;
}

async fn run_score_query_pipeline(
    spec: ScoreQuerySpec,
    params: PipelineParams<'_>,
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    mode: GameMode,
) {
    let PipelineParams {
        username,
        qq,
        beatmap_id,
        limit,
        limit_end,
        is_summary,
        filters,
    } = params;

    let is_self = username.is_none() && qq.is_none();

    let raw_limit = limit_end.unwrap_or(limit);
    let has_client_filter = filters.is_some_and(|f| !f.is_empty()) || beatmap_id.is_some();
    let api_limit = spec.api_fetch_limit.unwrap_or_else(|| {
        if has_client_filter {
            raw_limit.max(SCORE_API_FETCH_LIMIT)
        } else {
            raw_limit
        }
    });

    let (_user_id, resolved_username, user_stats, score_result) = if is_self {
        let (uid, name) = match ctx.resolve_binding(msg.user_id).await {
            Some(binding) => binding,
            None => {
                let _ = resp_tx
                    .send(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }
        };

        ctx.scheduler.trigger_update(uid, mode).await;

        let (stats_result, scores) = tokio::join!(
            api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, uid, mode),
            spec.fetch
                .call(&ctx.rate_limiter, &ctx.oauth, uid, mode, api_limit),
        );

        let user_stats = match stats_result {
            Ok(stats) => {
                cache_user_id(ctx, &stats).await;
                stats
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id = uid, "{}", log_fmt!("main.resolve_bound_user_failed"));
                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                return;
            }
        };

        (uid, name, user_stats, scores)
    } else {
        let (uid, name, user_stats) = match resolve_score_user(
            ctx,
            msg,
            &username.map(|s| s.to_string()),
            &qq,
            mode,
            resp_tx,
        )
        .await
        {
            Some(u) => u,
            None => return,
        };

        ctx.scheduler.trigger_update(uid, mode).await;

        let scores = spec
            .fetch
            .call(&ctx.rate_limiter, &ctx.oauth, uid, mode, api_limit)
            .await;

        (uid, name, user_stats, scores)
    };

    let mut scores = match score_result {
        Ok(scores) => scores,
        Err(err_msg) => {
            let _ = resp_tx.send(err_msg).await;
            return;
        }
    };

    if let Some(pp) = spec.post_process {
        pp(&mut scores);
    }

    if scores.is_empty() {
        let _ = resp_tx
            .send(user_str(spec.empty_msg_key).replace("{qq}", &msg.user_id.to_string()))
            .await;
        return;
    }

    ctx.last_beatmap
        .set(msg.group_id, scores[0].beatmap_id as u32);

    let mut indexed: Vec<(usize, Score)> = scores.drain(..).enumerate().collect();

    // PP 预补全：过滤器含 pp 条件时，先对 pp=None 的成绩做本地 PP 计算
    if filters.is_some_and(|f| f.iter().any(|s| s.starts_with("pp"))) {
        let enrich: Vec<usize> = indexed
            .iter()
            .enumerate()
            .filter(|(_, (_, s))| s.pp.is_none() && s.beatmap_id > 0)
            .map(|(i, _)| i)
            .collect();
        if !enrich.is_empty() {
            let futs: Vec<_> = enrich
                .iter()
                .map(|&i| {
                    let mut s = indexed[i].1.clone();
                    async move {
                        enrich_score_with_pp(&mut s, mode, true).await;
                        s
                    }
                })
                .collect();
            for (&i, s) in enrich.iter().zip(join_all(futs).await) {
                indexed[i].1 = s;
            }
        }
    }

    if let Some(bid) = beatmap_id {
        indexed.retain(|(_, s)| s.beatmap_id == bid);
        if indexed.is_empty() {
            send_no_match(resp_tx, msg.user_id, spec.noun_key).await;
            return;
        }
    }

    if let Some(filters) = filters {
        indexed.retain(|(_, s)| score_matches_filters(s, filters));
        if indexed.is_empty() {
            send_no_match(resp_tx, msg.user_id, spec.noun_key).await;
            return;
        }
    }

    if is_summary || limit_end.is_some() {
        let mut original_indices: Vec<usize> = indexed.iter().map(|(i, _)| *i).collect();
        scores = indexed.into_iter().map(|(_, s)| s).collect();

        if let Some(end) = limit_end {
            let start = (limit - 1) as usize;
            let end = end as usize;
            if start >= scores.len() {
                send_index_out_of_range(resp_tx, msg.user_id, spec.noun_key, limit, scores.len())
                    .await;
                return;
            }
            let end = end.min(scores.len());
            let _ = scores.drain(..start);
            scores.truncate(end - start);
            let _ = original_indices.drain(..start);
            original_indices.truncate(end - start);
        } else {
            if spec.truncate_bare_list {
                scores.truncate(limit as usize);
                original_indices.truncate(limit as usize);
            }
        }

        render_and_send_score_list(
            ctx,
            msg,
            resp_tx,
            &scores,
            &user_stats,
            &resolved_username,
            mode,
            spec.label_key,
            &original_indices,
        )
        .await;
    } else {
        scores = indexed.into_iter().map(|(_, s)| s).collect();
        let index = (limit - 1) as usize;
        if index >= scores.len() {
            send_index_out_of_range(resp_tx, msg.user_id, spec.noun_key, limit, scores.len()).await;
            return;
        }

        if spec.single_needs_backfill {
            let mode_str = mode.api_value().to_string();
            api::backfill_score_details(
                &ctx.rate_limiter,
                &ctx.oauth,
                &mut scores[index],
                &mode_str,
            )
            .await;
        }

        match spec.render {
            RenderOutput::SingleScoreCard => {
                render_and_send_single_score(SingleScoreRenderParams {
                    ctx,
                    msg,
                    resp_tx,
                    score: &scores[index],
                    mode,
                    user_stats: &user_stats,
                    position: Some(index),
                    label: user_str(spec.label_key),
                })
                .await;
            }
            RenderOutput::Audio => {
                audio::render_audio(ctx, msg, resp_tx, &scores[index], mode).await;
            }
            RenderOutput::BeatmapPreview => {
                preview::render_beatmap_preview_from_score(
                    ctx,
                    msg,
                    resp_tx,
                    &spec.preview_mods,
                    spec.preview_gif,
                    &spec.preview_times,
                    &scores[index],
                    mode,
                )
                .await;
            }
            RenderOutput::ScoreListCard => {
                tracing::error!(
                    "{}",
                    log_fmt!("pipeline.unexpected_score_list_card_in_single_path")
                );
                let _ = resp_tx
                    .send(user_str("error.render_failed").replace("{qq}", &msg.user_id.to_string()))
                    .await;
            }
        }
    }
}

/// 通过 beatmap_id 解析 beatmapset_id，按 beatmap_id 去重并发请求。
async fn fetch_beatmapset_id_dedup(
    ctx: &BotContext,
    beatmap_id: i64,
) -> Result<i64, DedupApiError> {
    let rl = ctx.rate_limiter.clone();
    let oauth = ctx.oauth.clone();
    beatmapset_dedup()
        .run_or_wait(beatmap_id, move || {
            let rl = rl.clone();
            let oauth = oauth.clone();
            async move {
                api::get_beatmapset_id(&rl, &oauth, beatmap_id)
                    .await
                    .map_err(|e| DedupApiError::from_api_error(&e))
            }
        })
        .await
}
