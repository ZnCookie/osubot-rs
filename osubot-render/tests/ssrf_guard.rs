use osubot_render::cache::is_blocked_url;

#[test]
fn blocks_ipv4_loopback() {
    assert!(is_blocked_url("http://127.0.0.1/foo.png"));
    assert!(is_blocked_url("http://127.255.255.254/foo.png"));
}

#[test]
fn blocks_ipv4_private() {
    assert!(is_blocked_url("http://10.0.0.1/foo.png"));
    assert!(is_blocked_url("http://192.168.1.1/foo.png"));
    assert!(is_blocked_url("http://172.16.0.1/foo.png"));
}

#[test]
fn blocks_ipv4_link_local_and_multicast() {
    assert!(is_blocked_url("http://169.254.169.254/foo.png"));
    assert!(is_blocked_url("http://224.0.0.1/foo.png"));
    assert!(is_blocked_url("http://255.255.255.255/foo.png"));
}

#[test]
fn blocks_ipv4_mapped_loopback() {
    assert!(is_blocked_url("http://[::ffff:127.0.0.1]/foo.png"));
}

#[test]
fn blocks_ipv4_mapped_private() {
    assert!(is_blocked_url("http://[::ffff:10.0.0.1]/foo.png"));
    assert!(is_blocked_url("http://[::ffff:192.168.1.1]/foo.png"));
    assert!(is_blocked_url("http://[::ffff:172.16.0.1]/foo.png"));
}

#[test]
fn blocks_ipv6_loopback_unique_local_link_local() {
    assert!(is_blocked_url("http://[::1]/foo.png"));
    assert!(is_blocked_url("http://[fc00::1]/foo.png"));
    assert!(is_blocked_url("http://[fd00::1]/foo.png"));
    assert!(is_blocked_url("http://[fe80::1]/foo.png"));
}

#[test]
fn blocks_6to4_prefix() {
    assert!(is_blocked_url("http://[2002:0a00:0001::]/foo.png"));
}

#[test]
fn blocks_localhost_and_dot_local() {
    assert!(is_blocked_url("http://localhost/foo.png"));
    assert!(is_blocked_url("http://printer.local/foo.png"));
}

#[test]
fn blocks_unparseable_url() {
    assert!(is_blocked_url("not a url"));
    assert!(is_blocked_url(""));
}

#[test]
fn allows_public_ipv4() {
    assert!(!is_blocked_url("http://1.1.1.1/foo.png"));
    assert!(!is_blocked_url("https://8.8.8.8/foo.png"));
}

#[test]
fn allows_public_ipv6() {
    assert!(!is_blocked_url("http://[2606:4700:4700::1111]/foo.png"));
    assert!(!is_blocked_url("https://[2001:4860:4860::8888]/foo.png"));
}

#[test]
fn allows_public_hostname() {
    assert!(!is_blocked_url("https://osu.ppy.sh/foo.png"));
    assert!(!is_blocked_url("https://a.ppy.sh/foo.png"));
}

#[tokio::test]
async fn test_inline_external_images_replaces_blocked_url_with_placeholder() {
    let html = r#"<p>hi</p><img src="http://127.0.0.1/x.png" alt="x"><p>bye</p>"#;
    let out = osubot_render::cache::inline_external_images(html).await;
    assert!(
        !out.contains("127.0.0.1"),
        "blocked URL must not appear in output: {out}"
    );
    assert!(
        out.contains("data:image/png;base64,"),
        "blocked <img> must be replaced by placeholder data URI: {out}"
    );
    assert!(
        out.contains("hi") && out.contains("bye"),
        "siblings preserved"
    );
}

#[tokio::test]
async fn test_inline_external_images_replaces_file_url_with_placeholder() {
    let html = r#"<img src="file:///etc/passwd">"#;
    let out = osubot_render::cache::inline_external_images(html).await;
    assert!(
        !out.contains("file://"),
        "file:// URL must be blocked: {out}"
    );
    assert!(out.contains("data:image/png;base64,"));
}

#[test]
fn test_is_blocked_url_blocks_nat64() {
    // RFC 6052 NAT64 prefix
    assert!(osubot_render::cache::is_blocked_url(
        "http://[64:ff9b::7f00:1]/x"
    ));
}

#[test]
fn test_is_blocked_url_blocks_teredo() {
    // RFC 4380 Teredo prefix
    assert!(osubot_render::cache::is_blocked_url("http://[2001::1]/x"));
}

#[test]
fn test_is_blocked_url_hostname_case_insensitive() {
    // RFC 3986 host part is case-insensitive
    assert!(osubot_render::cache::is_blocked_url("http://LocalHost/x"));
    assert!(osubot_render::cache::is_blocked_url("http://LOCALHOST/x"));
}
