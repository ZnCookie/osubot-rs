fn check_v4_private(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
}

pub fn is_blocked_url(url: &str) -> bool {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true,
    };
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return true;
    }
    let host_raw = match parsed.host_str() {
        Some(h) => h,
        None => return true,
    };
    let host_raw = host_raw.to_ascii_lowercase();
    let host = host_raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&host_raw);
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => check_v4_private(v4),
            std::net::IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return check_v4_private(v4);
                }
                if let Some(first) = v6.segments().first() {
                    if *first == 0x0064 && v6.segments()[1] == 0xff9b {
                        return true;
                    }
                }
                if v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0000 {
                    return true;
                }
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unicast_link_local()
                    || v6.is_unique_local()
                    || (v6.segments()[0] == 0x2002)
            }
        };
    }
    matches!(
        host,
        "localhost" | "0.0.0.0" | "::1" | "ip6-localhost" | "ip6-loopback"
    ) || host.ends_with(".local")
}

/// `reqwest::redirect::Policy` 在每次 `Location` 跳转时调用 `check` 校验。
/// 攻击者绕过 SSRF 的标准手法：302 → 内网。逐跳校验阻断此类旁路。
pub fn redirect_policy<F>(check: F) -> reqwest::redirect::Policy
where
    F: Fn(&reqwest::Url) -> bool + Send + Sync + 'static,
{
    reqwest::redirect::Policy::custom(move |attempt| {
        // 与 Policy::limited(5) 对齐：超过 5 次重定向后中断。
        // 真实攻击者需要 ≤ 5 跳可达内网，5 跳足够；放行第 6 跳无收益。
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        if check(attempt.url()) {
            attempt.error("redirect to blocked url")
        } else {
            attempt.follow()
        }
    })
}

/// Convenience wrapper for `redirect_policy` callbacks: takes a `&reqwest::Url`
/// and returns whether the URL is blocked. Performs the same scheme + host
/// checks as `is_blocked_url`. Use this from `http_client` constructors so
/// the SSRF predicate is identical across `osubot-core` and `osubot-render`.
pub fn is_blocked_redirect_url(url: &reqwest::Url) -> bool {
    is_blocked_url(url.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loopback() {
        assert!(is_blocked_url("http://127.0.0.1/"));
        assert!(is_blocked_url("http://localhost/"));
        assert!(is_blocked_url("http://[::1]/"));
    }

    #[test]
    fn blocks_private_ipv4() {
        assert!(is_blocked_url("http://10.0.0.1/"));
        assert!(is_blocked_url("http://192.168.1.1/"));
        assert!(is_blocked_url("http://172.16.0.1/"));
    }

    #[test]
    fn blocks_link_local() {
        assert!(is_blocked_url("http://169.254.1.1/"));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6() {
        assert!(is_blocked_url("http://[::ffff:127.0.0.1]/"));
        assert!(is_blocked_url("http://[::ffff:192.168.1.1]/"));
    }

    #[test]
    fn blocks_local_domains() {
        assert!(is_blocked_url("http://myhost.local/"));
        assert!(is_blocked_url("http://ip6-localhost/"));
    }

    #[test]
    fn allows_public_urls() {
        assert!(!is_blocked_url("https://example.com/"));
        assert!(!is_blocked_url("https://8.8.8.8/"));
        assert!(!is_blocked_url("https://osu.ppy.sh/api/v2/me"));
    }

    #[test]
    fn blocks_invalid_urls() {
        assert!(is_blocked_url("not-a-url"));
        assert!(is_blocked_url("file:///etc/passwd"));
    }

    #[test]
    fn blocks_non_http_schemes() {
        assert!(is_blocked_url("file:///etc/passwd"));
        assert!(is_blocked_url("ftp://example.com/"));
        assert!(is_blocked_url("gopher://example.com/"));
        assert!(is_blocked_url("data:text/html,<script>alert(1)</script>"));
    }

    #[test]
    fn redirect_policy_constructs_without_panic() {
        // reqwest 的 `Attempt` 类型所有字段都是私有的，单元测试无法直接
        // 构造一个可调用的 `Attempt` 来断言 `redirect_policy` 在每跳上的行为
        // （blocked URL、hop cap、内网目标）。redirect 跳转逻辑的端到端
        // 覆盖交给集成测试（`osubot-core` 与 `osubot-render` 各自的
        // `http_client` 在真实 HTTP 服务前的行为）。
        // 这里只验证 `redirect_policy` 自身可以构造出来且闭包签名匹配。
        let policy = redirect_policy(|u: &reqwest::Url| is_blocked_url(u.as_str()));
        let public = reqwest::Url::parse("https://example.com/").unwrap();
        let internal = reqwest::Url::parse("http://127.0.0.1/").unwrap();
        let _ = (policy, public, internal);
    }
}
