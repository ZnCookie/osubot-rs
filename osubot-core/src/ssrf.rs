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
}
