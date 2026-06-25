//! 分数查询与渲染。
//!
//! 行号注释约定：`L<n>-` / `L<a>-<b>` 标注紧随其后的代码块对应的行号范围，
//! 便于定位与维护。`FnOnce` 闭包传入 `RequestDedup::run_or_wait` 时，
//! 在闭包内重新 clone `ctx.rate_limiter` / `ctx.oauth` 以满足 `'static`。

use crate::score_filter::{score_matches_filters, ScoreQueryParams};
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
use tracing::{info, warn};

use osubot_core::OauthTokenCache;
use osubot_core::RateLimiter;

use crate::api_error_msg;
use crate::onebot::{send_group_msg_with_image, QQMessage};
use crate::{
    beatmap_scores_dedup, best_scores_dedup, score_by_id_dedup, score_by_id_err_msg, score_dedup,
    today_best_scores_dedup, DedupApiError, SCORE_API_FETCH_LIMIT,
};

mod plan;
mod render;

use plan::{process_scores, ScoreQueryPlan};
use render::{
    render_and_send_score_list, render_and_send_single_score, render_scores, render_single_score,
    SingleScoreRenderParams,
};

#[expect(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderOutput {
    SingleScoreCard,
    ScoreListCard,
    Audio,
    BeatmapPreview,
}

#[expect(dead_code)]
pub(crate) trait FetchFn: Send + Sync {
    fn call(
        &self,
        rate_limiter: &Arc<RateLimiter>,
        oauth: &Arc<OauthTokenCache>,
        user_id: i64,
        beatmap_id: Option<i64>,
        mode: GameMode,
        limit: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Score>, String>> + Send + '_>>;
}

#[expect(dead_code)]
fn make_fetch<F, Fut>(f: F) -> Box<dyn FetchFn>
where
    F: Fn(Arc<RateLimiter>, Arc<OauthTokenCache>, i64, Option<i64>, GameMode, u32) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: Future<Output = Result<Vec<Score>, String>> + Send + 'static,
{
    struct Impl<F>(F);
    impl<F, Fut> FetchFn for Impl<F>
    where
        F: Fn(Arc<RateLimiter>, Arc<OauthTokenCache>, i64, Option<i64>, GameMode, u32) -> Fut
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
            bid: Option<i64>,
            mode: GameMode,
            limit: u32,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Score>, String>> + Send + '_>> {
            Box::pin((self.0)(rl.clone(), oa.clone(), uid, bid, mode, limit))
        }
    }
    Box::new(Impl(f))
}

#[expect(dead_code)]
pub(crate) struct ScoreQuerySpec {
    pub(crate) fetch: Box<dyn FetchFn>,
    pub(crate) post_process: Option<fn(&mut Vec<Score>)>,
    pub(crate) render: RenderOutput,
    pub(crate) label_key: &'static str,
    pub(crate) noun_key: &'static str,
    pub(crate) empty_msg_key: &'static str,
    pub(crate) truncate_bare_list: bool,
    pub(crate) single_needs_backfill: bool,
}

#[expect(dead_code)]
struct PipelineParams<'a> {
    username: Option<&'a str>,
    qq: Option<i64>,
    beatmap_id: Option<i64>,
    score_id: Option<u64>,
    limit: u32,
    limit_end: Option<u32>,
    is_summary: bool,
    filters: Option<&'a [String]>,
}

const TODAY_BP_API_LIMIT: u32 = 200;

/// 发送错误消息到响应通道。
/// 用于消除 `let _ = resp_tx.send(...).await; return;` 样板。
/// 若通道已关闭（接收端 drop），记 warn 便于排查，不影响主流程。
async fn respond_err(resp_tx: &mpsc::Sender<String>, msg: impl Into<String>) {
    if resp_tx.send(msg.into()).await.is_err() {
        warn!("{}", log_fmt!("main.respond_err_send_failed"));
    }
}

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

pub(crate) async fn handle_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    params: ScoreQueryParams<'_>,
    mode: GameMode,
) {
    // L132-: handle_score_query 主流程
    //   L156-225: is_self 分支（已绑定用户）
    //   L228-285: 非 is_self 分支（指定用户/qq）
    //   L295-544: score_result 处理与渲染
    tracing::trace!("{}", log_fmt!("main.handle_score_query_start"));

    let is_self = params.username.is_none() && params.qq.is_none();
    let include_fails = !params.is_pass;
    let raw_limit = params.limit_end.unwrap_or(params.limit);
    let has_client_filter = params.filters.is_some_and(|f| !f.is_empty())
        || params.beatmap_id.is_some()
        || params.score_id.is_some();
    let api_limit = if has_client_filter {
        raw_limit.max(SCORE_API_FETCH_LIMIT)
    } else {
        raw_limit
    };
    let (user_id, resolved_username, user_stats, score_result) = if is_self {
        let (uid, name) = match ctx.resolve_binding(msg.user_id).await {
            Some(binding) => binding,
            None => {
                let _ = resp_tx
                    .send(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }
        };

        tracing::trace!(
            "{}",
            log_fmt!(
                "main.handle_score_query_bound",
                user_id = uid,
                username = name
            )
        );
        ctx.scheduler.trigger_update(uid, mode).await;

        // 第一个 future 直接借用 ctx 字段；第二个 future 需在 FnOnce 闭包内
        // 重新 clone（move 进 async 块以满足 'static）。
        let is_pass = params.is_pass;
        let qq = msg.user_id;

        let (stats_result, scores) = tokio::join!(
            api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, uid, mode),
            score_dedup().run_or_wait((uid, is_pass, api_limit, mode), move || {
                let rate_limiter = ctx.rate_limiter.clone();
                let oauth = ctx.oauth.clone();

                async move {
                    api::get_user_recent(&rate_limiter, &oauth, uid, mode, include_fails, api_limit, false)
                        .await
                        .map(Arc::new)
                        .map_err(|e| {
                            warn!(user_id = uid, mode = ?mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                            if !matches!(e, api::ApiError::NotFound | api::ApiError::RateLimitedWithRetryAfter(_) | api::ApiError::ClientRateLimited) {
                                tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                            }
                            api_error_msg(qq, &e)
                        })
                }
            }),
        );

        let user_stats = match stats_result {
            Ok(stats) => {
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
        let qq = msg.user_id;
        let (uid, name, user_stats) =
            match resolve_score_user(ctx, msg, params.username, params.qq, mode, resp_tx).await {
                Some(u) => {
                    tracing::trace!(
                        "{}",
                        log_fmt!(
                            "main.resolve_score_user_resolved",
                            user_id = u.0,
                            username = &u.1
                        )
                    );
                    u
                }
                None => {
                    tracing::warn!("{}", log_fmt!("main.resolve_score_user_none"));
                    return;
                }
            };

        ctx.scheduler.trigger_update(uid, mode).await;
        let dedup_key = (uid, params.is_pass, api_limit, mode);
        let dedup_mode = mode;

        tracing::trace!(
            "{}",
            log_fmt!(
                "main.fetch_scores",
                user_id = uid,
                mode = &format!("{:?}", mode),
                limit = api_limit
            )
        );
        let scores: Result<Arc<Vec<Score>>, String> = score_dedup()
            .run_or_wait(dedup_key, move || {
                let dedup_rate_limiter = ctx.rate_limiter.clone();
                let dedup_oauth = ctx.oauth.clone();

                async move {
                    api::get_user_recent(
                        &dedup_rate_limiter,
                        &dedup_oauth,
                        uid,
                        dedup_mode,
                        include_fails,
                        api_limit,
                        false,
                    )
                    .await
                    .map(Arc::new)
                    .map_err(|e| {
                        warn!(user_id = uid, mode = ?dedup_mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                        if !matches!(e, api::ApiError::NotFound | api::ApiError::RateLimitedWithRetryAfter(_) | api::ApiError::ClientRateLimited) {
                            tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                        }
                        api_error_msg(qq, &e)
                    })
                }
            })
            .await;

        (uid, name, user_stats, scores)
    };

    let dedup_username = resolved_username.clone();
    let qq = msg.user_id;

    match score_result {
        Ok(mut scores) => {
            if scores.is_empty() {
                let empty_msg = if include_fails {
                    user_str("query.no_records").replace("{qq}", &msg.user_id.to_string())
                } else {
                    user_str("query.no_records_pass").replace("{qq}", &msg.user_id.to_string())
                };
                let _ = resp_tx.send(empty_msg).await;
                return;
            }
            ctx.last_beatmap
                .set(msg.group_id, scores[0].beatmap_id as u32);

            if let Some(bid) = params.beatmap_id {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| s.beatmap_id == bid as i64);
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if let Some(sid) = params.score_id {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| s.score_id == sid as i64);
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if let Some(filters) = params.filters {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| score_matches_filters(s, filters));
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if params.is_single {
                let index = (params.limit - 1) as usize;
                if index >= scores.len() {
                    let _ = resp_tx
                        .send(
                            user_str("query.index_out_of_range")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{pos}", &params.limit.to_string())
                                .replace("{name}", user_str("query.noun_replay"))
                                .replace("{total}", &scores.len().to_string()),
                        )
                        .await;
                    return;
                }
                let score = &scores[index];
                render_and_send_single_score(SingleScoreRenderParams {
                    ctx,
                    msg,
                    resp_tx,
                    score,
                    mode,
                    user_stats: &user_stats,
                    position: Some(index),
                    label: if params.is_pass {
                        user_str("fmt.recent_pass")
                    } else {
                        user_str("fmt.recent_play")
                    },
                })
                .await;
            } else {
                if let Some(end) = params.limit_end {
                    let start = (params.limit - 1) as usize;
                    let end = end as usize;
                    if start >= scores.len() {
                        let _ = resp_tx
                            .send(
                                user_str("query.index_out_of_range")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{pos}", &params.limit.to_string())
                                    .replace("{name}", user_str("query.noun_replay"))
                                    .replace("{total}", &scores.len().to_string()),
                            )
                            .await;
                        return;
                    }
                    let end = end.min(scores.len());
                    let scores_arc = Arc::make_mut(&mut scores);
                    let _ = scores_arc.drain(..start);
                    scores_arc.truncate(end - start);
                }

                let scores_mut = Arc::make_mut(&mut scores);
                let cover_images: Vec<Option<image::DynamicImage>> =
                    futures_util::future::join_all(scores_mut.iter().map(|s| {
                        let url = s.cover_url.clone();
                        async move {
                            if url.is_empty() {
                                return None;
                            }
                            match osubot_render::cache::fetch_and_cache(
                                &url,
                                osubot_render::cache::http_client(),
                                false,
                            )
                            .await
                            {
                                Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                                Err(_) => None,
                            }
                        }
                    }))
                    .await;

                let avatar_url = format!("https://a.ppy.sh/{}", user_stats.user_id);
                let hero_cover_url = user_stats.cover_url.clone().unwrap_or_default();
                let user_global_rank = if user_stats.rank > 0 {
                    Some(user_stats.rank)
                } else {
                    None
                };
                let user_country_rank = if user_stats.country_rank > 0 {
                    Some(user_stats.country_rank)
                } else {
                    None
                };
                let change = ctx
                    .storage
                    .calculate_change(user_id, mode, &user_stats)
                    .await
                    .inspect_err(|e| {
                        tracing::warn!(
                            user_id = user_id,
                            mode = ?mode,
                            error = %e,
                            "{}",
                            log_fmt!("main.calculate_change_failed")
                        )
                    })
                    .ok()
                    .flatten();
                let pp_change = change.as_ref().and_then(|c| c.pp_change);
                let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
                let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);
                let score_label = if params.is_pass {
                    user_str("fmt.recent_pass")
                } else {
                    user_str("fmt.recent_play")
                };
                let score_count_text = user_str("fmt.score_count");
                let render_result = tokio::time::timeout(
                    std::time::Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
                    osubot_render::render_score_list_card(osubot_render::ScoreListCardParams {
                        user: osubot_render::UserContext {
                            username: &dedup_username,
                            mode,
                            user_pp: user_stats.pp,
                            user_global_rank,
                            user_country_rank,
                            country_code: &user_stats.country_code,
                            avatar_url: &avatar_url,
                            pp_change,
                            global_rank_change,
                            country_rank_change,
                        },
                        scores: &scores,
                        label: score_label,
                        count_text: score_count_text,
                        cover_images,
                        hero_cover_url: &hero_cover_url,
                        index_offset: 0,
                    }),
                )
                .await;

                match render_result {
                    Ok(Ok(jpeg_bytes)) => {
                        tracing::info!(
                            "{}",
                            log_fmt!("main.score_list_card_rendered", bytes = jpeg_bytes.len())
                        );
                        let jpeg = Arc::new(jpeg_bytes);
                        let write = ctx.write.clone();
                        let group_id = msg.group_id;
                        let resp_tx_img = resp_tx.clone();
                        tokio::spawn(async move {
                            if send_group_msg_with_image(&write, group_id, &jpeg)
                                .await
                                .is_err()
                            {
                                let _ = resp_tx_img
                                    .send(
                                        user_str("error.image_send_failed")
                                            .replace("{qq}", &qq.to_string()),
                                    )
                                    .await;
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "{}", log_fmt!("main.render_score_list_failed_text"));
                        let response = format_scores(&scores, &dedup_username, mode, score_label);
                        let _ = resp_tx.send(response).await;
                    }
                    Err(_) => {
                        warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
                        let response = format_scores(&scores, &dedup_username, mode, score_label);
                        let _ = resp_tx.send(response).await;
                    }
                }
            }
        }
        Err(err_msg) => {
            let _ = resp_tx.send(err_msg).await;
        }
    }
}

/// 描述 `!b` 与 `!t` 两条「最佳成绩类」查询之间的差异，
/// 公共流程由 [`handle_best_like_query`] 统一处理。
struct BestLikeQuerySpec {
    /// osu! API 抓取量。
    api_limit: u32,
    /// 去重实例（`!b` 与 `!t` 各用独立实例，缓存互不串）。
    dedup: &'static crate::BestScoresDedup,
    /// 是否过滤出最近 24 小时内的成绩（仅 `!t`）。
    filter_last_24h: bool,
    /// 空结果文案 key。
    empty_msg_key: &'static str,
    /// 卡片 label key。
    label_key: &'static str,
    /// 无区间裸列表时是否按 limit 截断（仅 `!t`）。
    truncate_bare_list: bool,
    /// 名词 key（filter 无匹配/索引越界时替换 {name}）。
    noun_key: &'static str,
}

/// [`handle_best_like_query`] 的入参，聚合避免过多函数参数。
struct BestLikeParams<'a> {
    ctx: &'a BotContext,
    msg: &'a QQMessage,
    resp_tx: &'a mpsc::Sender<String>,
    mode: GameMode,
    username: Option<&'a str>,
    qq: Option<i64>,
    limit: u32,
    limit_end: Option<u32>,
    is_summary: bool,
    filters: Option<&'a [String]>,
}

pub(crate) async fn handle_best_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    let (username, qq, limit, limit_end, is_summary, filters) = match cmd {
        Command::Best {
            username,
            qq,
            limit,
            limit_end,
            is_summary,
            filters,
            ..
        } => (
            username.as_deref(),
            *qq,
            *limit,
            *limit_end,
            *is_summary,
            filters.as_deref(),
        ),
        _ => return,
    };

    let raw_limit = limit_end.unwrap_or(limit);
    let has_client_filter = filters.is_some_and(|f| !f.is_empty());
    let api_limit = if has_client_filter {
        raw_limit.max(SCORE_API_FETCH_LIMIT)
    } else {
        raw_limit
    };

    handle_best_like_query(
        BestLikeParams {
            ctx,
            msg,
            resp_tx,
            mode,
            username,
            qq,
            limit,
            limit_end,
            is_summary,
            filters,
        },
        BestLikeQuerySpec {
            api_limit,
            dedup: best_scores_dedup(),
            filter_last_24h: false,
            empty_msg_key: "query.no_records_best",
            label_key: "fmt.best_score",
            truncate_bare_list: false,
            noun_key: "query.noun_best",
        },
    )
    .await;
}

pub(crate) async fn handle_today_bp_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    let (username, qq, limit, limit_end, is_summary, filters) = match cmd {
        Command::TodayBest {
            username,
            qq,
            limit,
            limit_end,
            is_summary,
            filters,
            ..
        } => (
            username.as_deref(),
            *qq,
            *limit,
            *limit_end,
            *is_summary,
            filters.as_deref(),
        ),
        _ => return,
    };

    handle_best_like_query(
        BestLikeParams {
            ctx,
            msg,
            resp_tx,
            mode,
            username,
            qq,
            limit,
            limit_end,
            is_summary,
            filters,
        },
        BestLikeQuerySpec {
            api_limit: TODAY_BP_API_LIMIT,
            dedup: today_best_scores_dedup(),
            filter_last_24h: true,
            empty_msg_key: "query.no_records_today_best",
            label_key: "fmt.today_best",
            truncate_bare_list: true,
            noun_key: "query.noun_today_best",
        },
    )
    .await;
}

async fn handle_best_like_query(params: BestLikeParams<'_>, spec: BestLikeQuerySpec) {
    let BestLikeParams {
        ctx,
        msg,
        resp_tx,
        mode,
        username,
        qq,
        limit,
        limit_end,
        is_summary,
        filters,
    } = params;

    let is_self = username.is_none() && qq.is_none();
    let api_limit = spec.api_limit;

    let (user_id, resolved_username, user_stats, score_result) = if is_self {
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

        let qq = msg.user_id;

        let (stats_result, scores) = tokio::join!(
            api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, uid, mode),
            spec.dedup.run_or_wait((uid, mode, api_limit), move || {
                let rate_limiter = ctx.rate_limiter.clone();
                let oauth = ctx.oauth.clone();

                async move {
                    api::get_user_best(&rate_limiter, &oauth, uid, mode, api_limit)
                        .await
                        .map_err(|e| {
                            warn!(user_id = uid, mode = ?mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                            if !matches!(e, api::ApiError::NotFound | api::ApiError::RateLimitedWithRetryAfter(_) | api::ApiError::ClientRateLimited) {
                                tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                            }
                            api_error_msg(qq, &e)
                        })
                }
            }),
        );

        let user_stats = match stats_result {
            Ok(stats) => {
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
        let qq_for_error = msg.user_id;
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
        let dedup_mode = mode;

        let scores: Result<Vec<Score>, String> = spec
            .dedup
            .run_or_wait((uid, mode, api_limit), move || {
                let dedup_rate_limiter = ctx.rate_limiter.clone();
                let dedup_oauth = ctx.oauth.clone();

                async move {
                    api::get_user_best(&dedup_rate_limiter, &dedup_oauth, uid, dedup_mode, api_limit)
                        .await
                        .map_err(|e| {
                            warn!(user_id = uid, mode = ?dedup_mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                            if !matches!(e, api::ApiError::NotFound | api::ApiError::RateLimitedWithRetryAfter(_) | api::ApiError::ClientRateLimited) {
                                tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                            }
                            api_error_msg(qq_for_error, &e)
                        })
                }
            })
            .await;

        (uid, name, user_stats, scores)
    };

    let dedup_username = resolved_username.clone();
    let qq = msg.user_id;

    match score_result {
        Ok(mut scores) => {
            // 仅 `!t`：过滤出最近 24 小时内打入最佳榜的成绩。
            // created_at 与 cutoff 都是绝对时间（UTC），时区无关。
            if spec.filter_last_24h {
                let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
                scores.retain(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s.created_at)
                        .ok()
                        .is_some_and(|dt| dt.naive_utc() > cutoff.naive_utc())
                });
            }

            if scores.is_empty() {
                let _ = resp_tx
                    .send(user_str(spec.empty_msg_key).replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }

            ctx.last_beatmap
                .set(msg.group_id, scores[0].beatmap_id as u32);

            if let Some(filters) = filters {
                scores.retain(|s| score_matches_filters(s, filters));
                if scores.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str(spec.noun_key)),
                        )
                        .await;
                    return;
                }
            }

            let mut index_offset: usize = 0;
            if is_summary {
                if let Some(end) = limit_end {
                    let start = (limit - 1) as usize;
                    let end = end as usize;
                    if start >= scores.len() {
                        let _ = resp_tx
                            .send(
                                user_str("query.index_out_of_range")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{pos}", &limit.to_string())
                                    .replace("{name}", user_str(spec.noun_key))
                                    .replace("{total}", &scores.len().to_string()),
                            )
                            .await;
                        return;
                    }
                    let end = end.min(scores.len());
                    index_offset = start;
                    let _ = scores.drain(..start);
                    scores.truncate(end - start);
                } else if spec.truncate_bare_list {
                    // 仅 `!t`：无区间裸列表最多展示 limit 条。
                    scores.truncate(limit as usize);
                }

                let cover_images: Vec<Option<image::DynamicImage>> =
                    futures_util::future::join_all(scores.iter().map(|s| {
                        let url = s.cover_url.clone();
                        async move {
                            if url.is_empty() {
                                return None;
                            }
                            match osubot_render::cache::fetch_and_cache(
                                &url,
                                osubot_render::cache::http_client(),
                                false,
                            )
                            .await
                            {
                                Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                                Err(_) => None,
                            }
                        }
                    }))
                    .await;

                let avatar_url = format!("https://a.ppy.sh/{}", user_stats.user_id);
                let hero_cover_url = user_stats.cover_url.clone().unwrap_or_default();
                let user_global_rank = if user_stats.rank > 0 {
                    Some(user_stats.rank)
                } else {
                    None
                };
                let user_country_rank = if user_stats.country_rank > 0 {
                    Some(user_stats.country_rank)
                } else {
                    None
                };
                let change = ctx
                    .storage
                    .calculate_change(user_id, mode, &user_stats)
                    .await
                    .inspect_err(|e| {
                        tracing::warn!(
                            user_id = user_id,
                            mode = ?mode,
                            error = %e,
                            "{}",
                            log_fmt!("main.calculate_change_failed")
                        )
                    })
                    .ok()
                    .flatten();
                let pp_change = change.as_ref().and_then(|c| c.pp_change);
                let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
                let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);
                let score_label = user_str(spec.label_key);
                let score_count_text = user_str("fmt.score_count");
                let render_result = tokio::time::timeout(
                    std::time::Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
                    osubot_render::render_score_list_card(osubot_render::ScoreListCardParams {
                        user: osubot_render::UserContext {
                            username: &dedup_username,
                            mode,
                            user_pp: user_stats.pp,
                            user_global_rank,
                            user_country_rank,
                            country_code: &user_stats.country_code,
                            avatar_url: &avatar_url,
                            pp_change,
                            global_rank_change,
                            country_rank_change,
                        },
                        scores: &scores,
                        label: score_label,
                        count_text: score_count_text,
                        cover_images,
                        hero_cover_url: &hero_cover_url,
                        index_offset,
                    }),
                )
                .await;

                match render_result {
                    Ok(Ok(jpeg_bytes)) => {
                        tracing::info!(
                            "{}",
                            log_fmt!("main.score_list_card_rendered", bytes = jpeg_bytes.len())
                        );
                        let jpeg = Arc::new(jpeg_bytes);
                        let write = ctx.write.clone();
                        let group_id = msg.group_id;
                        let resp_tx_img = resp_tx.clone();
                        tokio::spawn(async move {
                            if send_group_msg_with_image(&write, group_id, &jpeg)
                                .await
                                .is_err()
                            {
                                let _ = resp_tx_img
                                    .send(
                                        user_str("error.image_send_failed")
                                            .replace("{qq}", &qq.to_string()),
                                    )
                                    .await;
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "{}", log_fmt!("main.render_score_list_failed_text"));
                        let response = format_scores(&scores, &dedup_username, mode, score_label);
                        let _ = resp_tx.send(response).await;
                    }
                    Err(_) => {
                        warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
                        let response = format_scores(&scores, &dedup_username, mode, score_label);
                        let _ = resp_tx.send(response).await;
                    }
                }
            } else {
                let index = (limit - 1) as usize;
                if index >= scores.len() {
                    let _ = resp_tx
                        .send(
                            user_str("query.index_out_of_range")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{pos}", &limit.to_string())
                                .replace("{name}", user_str(spec.noun_key))
                                .replace("{total}", &scores.len().to_string()),
                        )
                        .await;
                    return;
                }
                // backfill 只处理将要展示的单条成绩
                {
                    let mode_str = mode.api_value().to_string();
                    api::backfill_score_details(
                        &ctx.rate_limiter,
                        &ctx.oauth,
                        &mut scores[index],
                        &mode_str,
                    )
                    .await;
                }
                let score = &scores[index];
                render_and_send_single_score(SingleScoreRenderParams {
                    ctx,
                    msg,
                    resp_tx,
                    score,
                    mode,
                    user_stats: &user_stats,
                    position: Some(index),
                    label: user_str(spec.label_key),
                })
                .await;
            }
        }
        Err(err_msg) => {
            let _ = resp_tx.send(err_msg).await;
        }
    }
}

struct ScoreQueryArgs {
    beatmap_id: i64,
    user_id: i64,
    username: String,
    limit: u32,
    limit_end: Option<u32>,
    mode: GameMode,
}

async fn run_score_query(
    plan: ScoreQueryPlan,
    args: ScoreQueryArgs,
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    user_stats: &UserStats,
    filters: Option<&[String]>,
) {
    let qq = msg.user_id;
    let mode = args.mode;
    let limit = args.limit;
    let limit_end = args.limit_end;
    let scores =
        match fetch_scores_with_dedup(ctx, args.beatmap_id, args.user_id, mode, plan.api_limit, qq)
            .await
        {
            Ok(s) => s,
            Err(err_msg) => return respond_err(resp_tx, err_msg).await,
        };

    if plan.bypass_filter {
        let score = match scores.into_iter().next() {
            Some(s) => s,
            None => {
                return respond_err(
                    resp_tx,
                    user_str("query.no_score_on_map").replace("{qq}", &qq.to_string()),
                )
                .await;
            }
        };
        ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
        return render_and_send_single_score(SingleScoreRenderParams {
            ctx,
            msg,
            resp_tx,
            score: &score,
            mode,
            user_stats,
            position: None,
            label: user_str("fmt.beatmap_score"),
        })
        .await;
    }

    let total_scores = scores.len();
    if total_scores == 0 {
        return respond_err(
            resp_tx,
            user_str("query.no_score_on_map").replace("{qq}", &qq.to_string()),
        )
        .await;
    }
    let scores = match process_scores(scores, filters, limit, limit_end) {
        Ok(s) => s,
        Err(key) => {
            let msg_text = user_str(key)
                .replace("{qq}", &qq.to_string())
                .replace("{pos}", &limit.to_string())
                .replace("{name}", user_str("query.noun_score"))
                .replace("{total}", &total_scores.to_string());
            return respond_err(resp_tx, msg_text).await;
        }
    };

    if plan.single_score {
        let score = match scores.into_iter().next() {
            Some(s) => s,
            None => {
                return respond_err(
                    resp_tx,
                    user_str("query.no_score_on_map").replace("{qq}", &qq.to_string()),
                )
                .await;
            }
        };
        return render_and_send_single_score(SingleScoreRenderParams {
            ctx,
            msg,
            resp_tx,
            score: &score,
            mode,
            user_stats,
            position: None,
            label: user_str("fmt.beatmap_score"),
        })
        .await;
    }

    if plan.is_all || limit_end.is_some() {
        render_scores(ctx, msg, resp_tx, &scores, user_stats, &args.username, mode).await;
    } else {
        let n = limit as usize;
        if scores.len() < n {
            return respond_err(
                resp_tx,
                user_str("query.index_out_of_range")
                    .replace("{qq}", &qq.to_string())
                    .replace("{pos}", &n.to_string())
                    .replace("{name}", user_str("query.noun_score"))
                    .replace("{total}", &scores.len().to_string()),
            )
            .await;
        }
        let Some(score) = scores.into_iter().nth(n - 1) else {
            // scores.len() < n 时已 early-return，但改为模式匹配消除 expect 依赖注释
            return;
        };
        render_single_score(ctx, msg, resp_tx, &score, user_stats, mode, n).await;
    }
}

pub(crate) async fn handle_beatmap_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    let (username, qq, beatmap_id, score_id, filters, limit, limit_end, is_all) = match cmd {
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
        } => (
            username.as_deref(),
            *qq,
            *beatmap_id,
            *score_id,
            filters.as_deref(),
            *limit,
            *limit_end,
            *is_all,
        ),
        _ => return,
    };

    if let Some(sid) = score_id {
        // L582-: 通过 score_id 查分流程
        info!(score_id = sid, "{}", log_fmt!("main.score_by_id"));
        let qq = msg.user_id;
        let sid_key = sid as i64;
        let score_result =
            score_by_id_dedup()
                .run_or_wait(sid_key, move || {
                    let rate_limiter = ctx.rate_limiter.clone();
                    let oauth = ctx.oauth.clone();

                    async move {
                        api::get_score_by_id(&rate_limiter, &oauth, sid).await.map_err(|e| {
                        if !matches!(e, api::ApiError::NotFound) {
                            warn!(error = ?e, "{}", log_fmt!("main.get_score_by_id_failed"));
                        }
                        DedupApiError::from_api_error(&e)
                    })
                    }
                })
                .await;
        let score = match score_result {
            Ok(s) => s,
            Err(e) => return respond_err(resp_tx, score_by_id_err_msg(qq, &e)).await,
        };
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
                ctx.scheduler.trigger_update(user_id, mode).await;
                stats
            }
            Err(e) => {
                if !matches!(e, api::ApiError::NotFound) {
                    warn!(user_id = user_id, error = ?e, "{}", log_fmt!("main.fetch_stats_score_id_failed"));
                }
                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                return;
            }
        };
        render_and_send_single_score(SingleScoreRenderParams {
            ctx,
            msg,
            resp_tx,
            score: &score,
            mode,
            user_stats: &user_stats,
            position: None,
            label: user_str("fmt.beatmap_score"),
        })
        .await;
        return;
    }

    let resolved_bid = match beatmap_id {
        Some(bid) => bid,
        None => match ctx.last_beatmap.get(msg.group_id) {
            Some(bid) => bid,
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

    info!(
        beatmap_id = resolved_bid,
        mode = ?mode,
        filters = ?filters,
        limit,
        is_all,
        "{}",
        log_fmt!("main.score_on_beatmap")
    );
    ctx.last_beatmap.set(msg.group_id, resolved_bid);

    let (_user_id, username_str, user_stats) = match resolve_score_user(
        ctx,
        msg,
        &username.map(|s| s.to_string()),
        &qq,
        mode,
        resp_tx,
    )
    .await
    {
        Some(result) => result,
        None => return,
    };

    ctx.scheduler.trigger_update(_user_id, mode).await;

    let plan = if is_all {
        let raw_api_limit = limit_end.or(if limit > 1 { Some(limit) } else { None });
        let api_limit = match (raw_api_limit, filters.is_some_and(|f| !f.is_empty())) {
            (Some(n), true) => Some(n.max(SCORE_API_FETCH_LIMIT)),
            (other, _) => other,
        };
        ScoreQueryPlan::list(api_limit)
    } else if limit == 1 && limit_end.is_none() {
        if filters.is_some_and(|f| !f.is_empty()) {
            ScoreQueryPlan::single_with_filters(SCORE_API_FETCH_LIMIT)
        } else {
            ScoreQueryPlan::single()
        }
    } else {
        let raw_limit = limit_end.unwrap_or(limit);
        let api_limit = if filters.is_some_and(|f| !f.is_empty()) {
            raw_limit.max(SCORE_API_FETCH_LIMIT)
        } else {
            raw_limit
        };
        ScoreQueryPlan::range(api_limit)
    };

    run_score_query(
        plan,
        ScoreQueryArgs {
            beatmap_id: resolved_bid as i64,
            user_id: _user_id,
            username: username_str,
            limit,
            limit_end,
            mode,
        },
        ctx,
        msg,
        resp_tx,
        &user_stats,
        filters,
    )
    .await;
}

async fn fetch_scores_with_dedup(
    ctx: &BotContext,
    resolved_bid: i64,
    user_id: i64,
    mode: GameMode,
    api_limit: Option<u32>,
    qq: i64,
) -> Result<Vec<Score>, String> {
    // FnOnce 闭包捕获 ctx，在闭包内 clone 以满足 'static
    let key = (user_id, resolved_bid, mode, api_limit);

    beatmap_scores_dedup()
        .run_or_wait(key, move || {
            let rate_limiter = ctx.rate_limiter.clone();
            let oauth = ctx.oauth.clone();

            async move {
                api::get_user_beatmap_scores_all(
                    &rate_limiter,
                    &oauth,
                    resolved_bid,
                    user_id,
                    mode,
                    api_limit,
                    false,
                )
                .await
                .map_err(|e| {
                    if !matches!(e, api::ApiError::NotFound) {
                        warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                    }
                    match e {
                        api::ApiError::NotFound => {
                            user_str("query.no_score_on_map").replace("{qq}", &qq.to_string())
                        }
                        other => api_error_msg(qq, &other),
                    }
                })
            }
        })
        .await
}

#[expect(dead_code)]
async fn run_score_query_pipeline(
    spec: ScoreQuerySpec,
    _cmd: &Command,
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
        score_id,
        limit,
        limit_end,
        is_summary,
        filters,
    } = params;

    let is_self = username.is_none() && qq.is_none();

    let raw_limit = limit_end.unwrap_or(limit);
    let has_client_filter = filters.is_some_and(|f| !f.is_empty())
        || beatmap_id.is_some()
        || score_id.is_some();
    let api_limit = if has_client_filter {
        raw_limit.max(SCORE_API_FETCH_LIMIT)
    } else {
        raw_limit
    };

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
                .call(&ctx.rate_limiter, &ctx.oauth, uid, None, mode, api_limit),
        );

        let user_stats = match stats_result {
            Ok(stats) => {
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
        let (uid, name, user_stats) =
            match resolve_score_user(ctx, msg, &username.map(|s| s.to_string()), &qq, mode, resp_tx)
                .await
            {
                Some(u) => u,
                None => return,
            };

        ctx.scheduler.trigger_update(uid, mode).await;

        let scores = spec
            .fetch
            .call(&ctx.rate_limiter, &ctx.oauth, uid, None, mode, api_limit)
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

    if let Some(bid) = beatmap_id {
        scores.retain(|s| s.beatmap_id == bid);
        if scores.is_empty() {
            let _ = resp_tx
                .send(
                    user_str("query.no_match")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{name}", user_str(spec.noun_key)),
                )
                .await;
            return;
        }
    }

    if let Some(sid) = score_id {
        scores.retain(|s| s.score_id == sid as i64);
        if scores.is_empty() {
            let _ = resp_tx
                .send(
                    user_str("query.no_match")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{name}", user_str(spec.noun_key)),
                )
                .await;
            return;
        }
    }

    if let Some(filters) = filters {
        scores.retain(|s| score_matches_filters(s, filters));
        if scores.is_empty() {
            let _ = resp_tx
                .send(
                    user_str("query.no_match")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{name}", user_str(spec.noun_key)),
                )
                .await;
            return;
        }
    }

    if is_summary || limit_end.is_some() {
        if let Some(end) = limit_end {
            let start = (limit - 1) as usize;
            let end = end as usize;
            if start >= scores.len() {
                let _ = resp_tx
                    .send(
                        user_str("query.index_out_of_range")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{pos}", &limit.to_string())
                            .replace("{name}", user_str(spec.noun_key))
                            .replace("{total}", &scores.len().to_string()),
                    )
                    .await;
                return;
            }
            let end = end.min(scores.len());
            let _ = scores.drain(..start);
            scores.truncate(end - start);
        } else if spec.truncate_bare_list {
            scores.truncate(limit as usize);
        }

        render_and_send_score_list(ctx, msg, resp_tx, &scores, &user_stats, &resolved_username, mode)
            .await;
    } else {
        let index = (limit - 1) as usize;
        if index >= scores.len() {
            let _ = resp_tx
                .send(
                    user_str("query.index_out_of_range")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{pos}", &limit.to_string())
                        .replace("{name}", user_str(spec.noun_key))
                        .replace("{total}", &scores.len().to_string()),
                )
                .await;
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
                render_audio(ctx, msg, resp_tx, &scores[index], mode, &user_stats).await;
            }
            RenderOutput::BeatmapPreview => {
                render_beatmap_preview_from_score(ctx, msg, resp_tx, &scores[index], mode, &user_stats)
                    .await;
            }
            RenderOutput::ScoreListCard => {
                unreachable!()
            }
        }
    }
}

#[expect(dead_code)]
async fn render_audio(
    _ctx: &BotContext,
    _msg: &QQMessage,
    _resp_tx: &mpsc::Sender<String>,
    _score: &Score,
    _mode: GameMode,
    _user_stats: &UserStats,
) {
    todo!()
}

#[expect(dead_code)]
async fn render_beatmap_preview_from_score(
    _ctx: &BotContext,
    _msg: &QQMessage,
    _resp_tx: &mpsc::Sender<String>,
    _score: &Score,
    _mode: GameMode,
    _user_stats: &UserStats,
) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_respond_err_sends_and_returns() {
        let (tx, mut rx) = mpsc::channel::<String>(1);
        respond_err(&tx, "error message").await;
        let received = rx.recv().await;
        assert_eq!(received.as_deref(), Some("error message"));
    }
}
