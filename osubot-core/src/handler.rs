use crate::api::{self, ApiError};
use crate::dedup::RequestDedup;
use crate::highlight::{format_highlight, get_highlight, HighlightError};
use crate::response::format_stats_with_change;
use crate::storage::Storage;
use crate::types::{Command, GameMode, QQMessage, ScoreCard};
use crate::{OauthTokenCache, RateLimiter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::{error, info, warn};

pub enum CommandResult {
    Text(String),
    ProfileCard(ProfileCardData),
    ScoreCard(Box<ScoreCard>),
    None,
}

#[derive(Clone)]
pub struct ProfileCardData {
    pub html: String,
    pub profile_hue: u16,
    pub avatar_url: String,
    pub username: String,
}

pub type GroupMemberFetcher = Arc<
    dyn Fn(i64) -> Pin<Box<dyn Future<Output = Result<Vec<i64>, String>> + Send>> + Send + Sync,
>;

pub struct HandlerContext {
    pub storage: Arc<Storage>,
    pub oauth: Arc<OauthTokenCache>,
    pub rate_limiter: Arc<RateLimiter>,
    pub trigger_update: Option<Arc<dyn Fn(i64) + Send + Sync>>,
    pub fetch_group_members: Option<GroupMemberFetcher>,
}

type ProfileDedup = RequestDedup<(i64, GameMode), ProfileCardData, String>;

fn profile_dedup() -> &'static ProfileDedup {
    static DEDUP: OnceLock<ProfileDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

type ScoreCardDedup = RequestDedup<(i64, GameMode, bool), Box<ScoreCard>, String>;

fn score_card_dedup() -> &'static ScoreCardDedup {
    static DEDUP: OnceLock<ScoreCardDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

pub async fn handle_command(
    ctx: HandlerContext,
    msg: QQMessage,
    irc_nickname: Option<String>,
) -> CommandResult {
    let cmd = match crate::commands::parse_command(&msg.message, msg.mentioned_user_id) {
        Some(cmd) => cmd,
        None => return CommandResult::None,
    };

    match cmd {
        Command::QuerySelf { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "QuerySelf command");
            match ctx.storage.get_binding(msg.user_id) {
                Ok(Some((user_id, current_username))) => {
                    if let Some(ref trigger) = ctx.trigger_update {
                        trigger(user_id);
                    }
                    match api::fetch_user_stats_by_user_id(
                        &ctx.rate_limiter,
                        &ctx.oauth,
                        user_id,
                        mode,
                    )
                    .await
                    {
                        Ok(stats) => {
                            if stats.username != current_username {
                                ctx.storage
                                    .update_binding_username(msg.user_id, &stats.username)
                                    .ok();
                            }
                            ctx.storage.set_user_id(&stats.username, user_id).ok();
                            let change = ctx
                                .storage
                                .calculate_change(user_id, mode, &stats)
                                .ok()
                                .flatten();
                            info!(user_id = user_id, username = %stats.username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QuerySelf success");
                            CommandResult::Text(format_stats_with_change(&stats, &change, mode))
                        }
                        Err(e) => {
                            warn!(user_id = user_id, mode = ?mode, error = ?e, "QuerySelf failed");
                            CommandResult::Text(match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            })
                        }
                    }
                }
                Ok(None) => {
                    info!(user_id = msg.user_id, "QuerySelf but no binding");
                    CommandResult::Text("请先绑定 osu! 用户名，使用 绑定 <用户名>".to_string())
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "QuerySelf database error");
                    CommandResult::Text("数据库错误".to_string())
                }
            }
        }
        Command::QueryUser { username, mode } => {
            info!(group_id = msg.group_id, username = %username, mode = ?mode, "QueryUser command");
            match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, &username, mode)
                .await
            {
                Ok(stats) => {
                    ctx.storage.set_user_id(&stats.username, stats.user_id).ok();
                    if stats.username != username {
                        ctx.storage.set_user_id(&username, stats.user_id).ok();
                    }
                    if let Some(ref trigger) = ctx.trigger_update {
                        trigger(stats.user_id);
                    }
                    let change = ctx
                        .storage
                        .calculate_change(stats.user_id, mode, &stats)
                        .ok()
                        .flatten();
                    info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QueryUser success");
                    CommandResult::Text(format_stats_with_change(&stats, &change, mode))
                }
                Err(e) => {
                    warn!(username = %username, mode = ?mode, error = ?e, "QueryUser failed");
                    CommandResult::Text(match e {
                        ApiError::NotFound => "未找到该用户".to_string(),
                        ApiError::MissingApiKey => "API Key 未配置".to_string(),
                        ApiError::OAuthError => "OAuth 认证失败".to_string(),
                        ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    })
                }
            }
        }
        Command::QueryMentionedUser { qq, mode } => {
            info!(qq = qq, group_id = msg.group_id, mode = ?mode, "QueryMentionedUser command");
            match ctx.storage.get_binding(qq) {
                Ok(Some((user_id, current_username))) => {
                    if let Some(ref trigger) = ctx.trigger_update {
                        trigger(user_id);
                    }
                    match api::fetch_user_stats_by_user_id(
                        &ctx.rate_limiter,
                        &ctx.oauth,
                        user_id,
                        mode,
                    )
                    .await
                    {
                        Ok(stats) => {
                            if stats.username != current_username {
                                ctx.storage
                                    .update_binding_username(qq, &stats.username)
                                    .ok();
                            }
                            ctx.storage.set_user_id(&stats.username, user_id).ok();
                            let change = ctx
                                .storage
                                .calculate_change(user_id, mode, &stats)
                                .ok()
                                .flatten();
                            info!(user_id = user_id, username = %stats.username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QueryMentionedUser success");
                            CommandResult::Text(format_stats_with_change(&stats, &change, mode))
                        }
                        Err(e) => {
                            warn!(user_id = user_id, mode = ?mode, error = ?e, "QueryMentionedUser failed");
                            CommandResult::Text(match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            })
                        }
                    }
                }
                Ok(None) => {
                    info!(qq = qq, "QueryMentionedUser but no binding");
                    CommandResult::Text(
                        "该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定".to_string(),
                    )
                }
                Err(_) => {
                    error!(qq = qq, "QueryMentionedUser database error");
                    CommandResult::Text("数据库错误".to_string())
                }
            }
        }
        Command::Bind { username } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, username = %username, "Bind command");
            match ctx.storage.get_binding(msg.user_id) {
                Ok(Some((_, existing_username))) => {
                    info!(user_id = msg.user_id, existing = %existing_username, "Bind but already bound");
                    CommandResult::Text(format!(
                        "你已经绑定为{},如需修改请先解绑",
                        existing_username
                    ))
                }
                Ok(None) => {
                    if let Some(nickname) = irc_nickname {
                        match ctx.storage.has_pending_bind(msg.user_id) {
                            Ok(true) => CommandResult::Text(
                                "你已有进行中的绑定请求，请等待当前验证码过期后再试".to_string(),
                            ),
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to check pending bind");
                                CommandResult::Text("绑定失败，请稍后重试".to_string())
                            }
                            _ => {
                                match ctx.storage.add_pending_bind(
                                    msg.user_id,
                                    msg.group_id,
                                    &username,
                                ) {
                                    Ok(code) => {
                                        info!(user_id = msg.user_id, username = %username, code = %code, "Pending bind created");
                                        CommandResult::Text(format!("您的验证码是 {}，请在两分钟内通过osu!发送私信给 {} 来完成验证", code, nickname))
                                    }
                                    Err(_) => {
                                        error!(
                                            user_id = msg.user_id,
                                            "Failed to create pending bind"
                                        );
                                        CommandResult::Text("绑定失败，请稍后重试".to_string())
                                    }
                                }
                            }
                        }
                    } else {
                        match api::get_user_info(&ctx.rate_limiter, &ctx.oauth, &username).await {
                            Ok(Some(user_info)) => {
                                if let Err(e) = ctx.storage.set_user_id(&username, user_info.id) {
                                    warn!("Failed to cache user_id for {username}: {e}");
                                }
                                match ctx.storage.bind(
                                    msg.user_id,
                                    user_info.id,
                                    &user_info.username,
                                ) {
                                    Ok(Ok(())) => {
                                        info!(user_id = msg.user_id, username = %user_info.username, "Bind success");
                                        CommandResult::Text(format!(
                                            "成功绑定为{}",
                                            user_info.username
                                        ))
                                    }
                                    Ok(Err(bound_qq)) => {
                                        info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "Bind failed - username already bound");
                                        CommandResult::Text("该 osu! 用户已绑定其他QQ".to_string())
                                    }
                                    Err(_) => {
                                        error!(user_id = msg.user_id, username = %username, "Bind failed");
                                        CommandResult::Text("绑定失败，请稍后重试".to_string())
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(username = %username, "Bind but user not found");
                                CommandResult::Text("未找到该 osu! 用户".to_string())
                            }
                            Err(e) => {
                                warn!(username = %username, error = ?e, "Bind - user info check failed");
                                CommandResult::Text(match e {
                                    ApiError::NotFound => "未找到该用户".to_string(),
                                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                    ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                    _ => "查询失败，请稍后重试".to_string(),
                                })
                            }
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Bind database error");
                    CommandResult::Text("数据库错误".to_string())
                }
            }
        }
        Command::Unbind => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "Unbind command"
            );
            match ctx.storage.get_pending_unbind(msg.user_id) {
                Ok(Some(_)) => match ctx.storage.unbind(msg.user_id) {
                    Ok(_) => {
                        ctx.storage.remove_pending_unbind(msg.user_id).ok();
                        info!(user_id = msg.user_id, "Unbind success");
                        CommandResult::Text("解绑成功".to_string())
                    }
                    Err(_) => {
                        error!(user_id = msg.user_id, "Unbind failed");
                        CommandResult::Text("解绑失败，请稍后重试".to_string())
                    }
                },
                Ok(None) => match ctx.storage.get_binding(msg.user_id) {
                    Ok(Some((_, current_username))) => {
                        ctx.storage.set_pending_unbind(msg.user_id).ok();
                        info!(user_id = msg.user_id, username = %current_username, "Unbind confirmation requested");
                        CommandResult::Text(format!(
                            "确定要解除绑定 {} 吗？回复\"解绑\"确认",
                            current_username
                        ))
                    }
                    Ok(None) => {
                        info!(user_id = msg.user_id, "Unbind but no binding");
                        CommandResult::Text("你还没有绑定任何 osu! 用户".to_string())
                    }
                    Err(_) => {
                        error!(user_id = msg.user_id, "Unbind database error");
                        CommandResult::Text("数据库错误".to_string())
                    }
                },
                Err(_) => {
                    error!(user_id = msg.user_id, "Unbind pending check error");
                    CommandResult::Text("数据库错误".to_string())
                }
            }
        }
        Command::Highlight { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "Highlight command");
            let group_member_ids = match &ctx.fetch_group_members {
                Some(fetcher) => match fetcher(msg.group_id).await {
                    Ok(ids) => ids,
                    Err(e) => {
                        warn!(group_id = msg.group_id, error = %e, "Failed to fetch group member list for Highlight");
                        return CommandResult::Text("获取群成员失败".to_string());
                    }
                },
                None => Vec::new(),
            };
            let group_members: std::collections::HashSet<i64> =
                group_member_ids.iter().copied().collect();
            let all_bindings = match ctx.storage.get_all_user_bindings() {
                Ok(bindings) => bindings,
                Err(_) => {
                    error!("Highlight failed to get bindings");
                    return CommandResult::Text("数据库错误".to_string());
                }
            };
            let group_bindings: Vec<(i64, i64, String)> = all_bindings
                .into_iter()
                .filter(|(qq, _, _)| group_members.contains(qq))
                .collect();
            if group_bindings.is_empty() {
                return CommandResult::Text("你群根本没有人绑定 osu! 账号".to_string());
            }
            match get_highlight(
                &ctx.storage,
                &ctx.rate_limiter,
                &ctx.oauth,
                &group_bindings,
                mode,
            )
            .await
            {
                Ok(result) => CommandResult::Text(format_highlight(&result)),
                Err(e) => {
                    warn!(error = ?e, "Highlight fetch failed");
                    CommandResult::Text(match e {
                        HighlightError::NoData => "你群根本没有人屙屎。".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    })
                }
            }
        }
        Command::ProfileCard { username, qq } => {
            let target_user_id = match username {
                Some(ref name) => {
                    if let Ok(Some(cached_id)) = ctx.storage.get_user_id(name) {
                        info!(username = %name, user_id = cached_id, "ProfileCard resolved from local cache");
                        cached_id
                    } else {
                        match api::fetch_user_stats_by_username(
                            &ctx.rate_limiter,
                            &ctx.oauth,
                            name,
                            GameMode::Osu,
                        )
                        .await
                        {
                            Ok(stats) => {
                                info!(username = %name, user_id = stats.user_id, "ProfileCard resolved by username");
                                ctx.storage.set_user_id(&stats.username, stats.user_id).ok();
                                stats.user_id
                            }
                            Err(e) => {
                                warn!(username = %name, error = ?e, "ProfileCard username resolution failed");
                                return CommandResult::Text(match e {
                                    ApiError::NotFound => "未找到该用户".to_string(),
                                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                    ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                    _ => "查询失败，请稍后重试".to_string(),
                                });
                            }
                        }
                    }
                }
                None => match qq {
                    Some(mentioned_qq) => match ctx.storage.get_binding(mentioned_qq) {
                        Ok(Some((user_id, current_username))) => {
                            info!(qq = mentioned_qq, osu_id = user_id, username = %current_username, "ProfileCard mention");
                            user_id
                        }
                        Ok(None) => {
                            info!(qq = mentioned_qq, "ProfileCard mention but no binding");
                            return CommandResult::Text(
                                "该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定"
                                    .to_string(),
                            );
                        }
                        Err(_) => {
                            error!(qq = mentioned_qq, "ProfileCard mention database error");
                            return CommandResult::Text("数据库错误".to_string());
                        }
                    },
                    None => match ctx.storage.get_binding(msg.user_id) {
                        Ok(Some((user_id, current_username))) => {
                            info!(user_id = msg.user_id, osu_id = user_id, username = %current_username, "ProfileCard self");
                            user_id
                        }
                        Ok(None) => {
                            return CommandResult::Text(
                                "请先绑定 osu! 用户名，或使用 !profile <用户名> 查询他人"
                                    .to_string(),
                            );
                        }
                        Err(_) => {
                            error!(user_id = msg.user_id, "ProfileCard database error");
                            return CommandResult::Text("数据库错误".to_string());
                        }
                    },
                },
            };
            info!(user_id = target_user_id, qq = ?qq, "ProfileCard command");
            let dedup_rate_limiter = ctx.rate_limiter.clone();
            let dedup_oauth = ctx.oauth.clone();
            let fetch_result = profile_dedup()
                .run_or_wait((target_user_id, GameMode::Osu), move || async move {
                    let profile = api::fetch_user_profile(
                        &dedup_rate_limiter,
                        &dedup_oauth,
                        target_user_id,
                        GameMode::Osu,
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => "未找到该用户".to_string(),
                        ApiError::MissingApiKey => "API Key 未配置".to_string(),
                        ApiError::OAuthError => "OAuth 认证失败".to_string(),
                        ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    })?;
                    info!(
                        user_id = target_user_id,
                        html_len = profile.html.len(),
                        hue = profile.profile_hue,
                        "ProfileCard HTML fetched"
                    );
                    Ok(ProfileCardData {
                        html: profile.html,
                        profile_hue: profile.profile_hue,
                        avatar_url: profile.avatar_url,
                        username: profile.username,
                    })
                })
                .await;
            match fetch_result {
                Ok(data) => {
                    info!(user_id = target_user_id, "ProfileCard data ready");
                    CommandResult::ProfileCard(data)
                }
                Err(msg) => {
                    warn!(user_id = target_user_id, msg = %msg, "ProfileCard failed");
                    CommandResult::Text(msg)
                }
            }
        }
        Command::ScoreCard {
            username,
            mode,
            include_fails,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, include_fails = include_fails, "ScoreCard command");
            let user_id = match username {
                Some(ref uname) => {
                    match api::fetch_user_stats_by_username(
                        &ctx.rate_limiter,
                        &ctx.oauth,
                        uname,
                        mode,
                    )
                    .await
                    {
                        Ok(stats) => stats.user_id,
                        Err(e) => {
                            warn!(username = %uname, mode = ?mode, error = ?e, "ScoreCard username lookup failed");
                            return CommandResult::Text(match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            });
                        }
                    }
                }
                None => match ctx.storage.get_binding(msg.user_id) {
                    Ok(Some((uid, _))) => uid,
                    Ok(None) => return CommandResult::Text("请先绑定 osu! 账号".to_string()),
                    Err(_) => return CommandResult::Text("数据库错误".to_string()),
                },
            };
            let dedup_rate_limiter = ctx.rate_limiter.clone();
            let dedup_oauth = ctx.oauth.clone();
            let fetch_result = score_card_dedup()
                .run_or_wait((user_id, mode, include_fails), move || async move {
                    let scores = api::get_user_recent(
                        &dedup_rate_limiter,
                        &dedup_oauth,
                        user_id,
                        mode,
                        include_fails,
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => "未找到成绩".to_string(),
                        ApiError::OAuthError => "OAuth 认证失败".to_string(),
                        ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    })?;
                    let score = scores
                        .into_iter()
                        .next()
                        .ok_or_else(|| "没有找到成绩".to_string())?;
                    Ok(Box::new(ScoreCard::from(score)))
                })
                .await;
            match fetch_result {
                Ok(score_card) => {
                    info!(user_id = user_id, "ScoreCard data ready");
                    CommandResult::ScoreCard(score_card)
                }
                Err(msg) => {
                    warn!(user_id = user_id, msg = %msg, "ScoreCard API call failed");
                    CommandResult::Text(msg)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Command;

    #[test]
    fn test_parse_profile_card_self() {
        let cmd = crate::commands::parse_command("!profile", None);
        assert!(matches!(
            cmd,
            Some(Command::ProfileCard {
                username: None,
                qq: None,
            })
        ));
    }

    #[test]
    fn test_parse_score_card_self() {
        let cmd = crate::commands::parse_command("!pr", None);
        assert!(matches!(
            cmd,
            Some(Command::ScoreCard {
                username: None,
                mode: GameMode::Osu,
                include_fails: false,
            })
        ));
    }

    #[test]
    fn test_parse_score_card_re_with_mode() {
        let cmd = crate::commands::parse_command("!re :1", None);
        assert!(matches!(
            cmd,
            Some(Command::ScoreCard {
                username: None,
                mode: GameMode::Taiko,
                include_fails: true,
            })
        ));
    }

    #[test]
    fn test_parse_score_card_with_username() {
        let cmd = crate::commands::parse_command("!pr ZnCookie", None);
        assert!(matches!(
            cmd,
            Some(Command::ScoreCard {
                username: Some(ref u),
                mode: GameMode::Osu,
                include_fails: false,
            }) if u == "ZnCookie"
        ));
    }

    #[test]
    fn test_parse_score_card_re_with_username_and_mode() {
        let cmd = crate::commands::parse_command("!re user :3", None);
        assert!(matches!(
            cmd,
            Some(Command::ScoreCard {
                username: Some(ref u),
                mode: GameMode::Mania,
                include_fails: true,
            }) if u == "user"
        ));
    }

    #[test]
    fn test_parse_bind() {
        let cmd = crate::commands::parse_command("绑定 ZnCookie", None);
        assert!(matches!(
            cmd,
            Some(Command::Bind { ref username }) if username == "ZnCookie"
        ));
    }

    #[test]
    fn test_parse_unbind() {
        let cmd = crate::commands::parse_command("解绑", None);
        assert!(matches!(cmd, Some(Command::Unbind)));
    }

    #[test]
    fn test_parse_highlight() {
        let cmd = crate::commands::parse_command("今日高光", None);
        assert!(matches!(
            cmd,
            Some(Command::Highlight { mode: GameMode::Osu })
        ));
    }

    #[test]
    fn test_parse_no_command() {
        let cmd = crate::commands::parse_command("hello world", None);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_reject_profile_prefix_greedy() {
        let cmd = crate::commands::parse_command("!profileX", None);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_reject_pr_prefix_greedy() {
        let cmd = crate::commands::parse_command("!pre", None);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_reject_re_prefix_greedy() {
        let cmd = crate::commands::parse_command("!red", None);
        assert!(cmd.is_none());
    }
}
