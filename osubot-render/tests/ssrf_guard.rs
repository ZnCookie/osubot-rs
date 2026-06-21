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
