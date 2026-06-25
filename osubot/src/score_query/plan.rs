use super::*;

/// 描述单次谱面分数查询的 fetch 配置。
/// 用于将 is_all / limit==1 / else 三分支的参数差异封装到一个类型。
#[allow(dead_code)]
pub(super) struct ScoreQueryPlan {
    /// osu! API 单次请求的最大数量。`None` 表示使用 API 默认。
    pub(super) api_limit: Option<u32>,
    /// 是否跳过过滤阶段（limit==1 + 无 filters 时为 true）
    pub(super) bypass_filter: bool,
    /// 是否单分模式（limit==1 + limit_end 为 None）
    pub(super) single_score: bool,
    /// 是否 `!sb *`（无 limit_end 时也按列表渲染）
    pub(super) is_all: bool,
}

impl ScoreQueryPlan {
    /// `!sb` 不带 limit_end 的默认单分查询。
    /// `api_limit: Some(1)` 让 `/all` 端点只返回首条成绩，避免无意义地传输最多 50 条。
    #[allow(dead_code)]
    pub(super) fn single() -> Self {
        Self {
            api_limit: Some(1),
            bypass_filter: true,
            single_score: true,
            is_all: false,
        }
    }

    /// `!sb <n>` 单分查询，可能带 filters。
    #[allow(dead_code)]
    pub(super) fn single_with_filters(api_limit: u32) -> Self {
        Self {
            api_limit: Some(api_limit),
            bypass_filter: false,
            single_score: true,
            is_all: false,
        }
    }

    /// `!sb *` 列出所有分（按 limit / limit_end 截取）。
    #[allow(dead_code)]
    pub(super) fn list(api_limit: Option<u32>) -> Self {
        Self {
            api_limit,
            bypass_filter: false,
            single_score: false,
            is_all: true,
        }
    }

    /// `!sb [n, m]` 范围查询。
    #[allow(dead_code)]
    pub(super) fn range(api_limit: u32) -> Self {
        Self {
            api_limit: Some(api_limit),
            bypass_filter: false,
            single_score: false,
            is_all: false,
        }
    }
}

#[allow(dead_code)]
fn filter_scores(scores: Vec<Score>, filters: Option<&[String]>) -> Vec<Score> {
    if let Some(filters) = filters {
        scores
            .into_iter()
            .filter(|s| score_matches_filters(s, filters))
            .collect()
    } else {
        scores
    }
}

#[allow(dead_code)]
pub(super) fn process_scores(
    scores: Vec<Score>,
    filters: Option<&[String]>,
    limit: u32,
    limit_end: Option<u32>,
) -> Result<Vec<Score>, &'static str> {
    let mut scores = filter_scores(scores, filters);
    if scores.is_empty() {
        return Err("query.no_match");
    }

    if let Some(end) = limit_end {
        let start = (limit - 1) as usize;
        let end = end as usize;
        if start >= scores.len() {
            return Err("query.index_out_of_range");
        }
        let end = end.min(scores.len());
        let _ = scores.drain(..start);
        scores.truncate(end - start);
        if scores.is_empty() {
            return Err("query.index_out_of_range");
        }
    }

    Ok(scores)
}
