pub mod auth;
pub mod cache;
pub mod catalog;
pub mod error;
pub mod jwt;
pub mod stream;
pub mod turnstile;
pub mod wire;

pub const USER_AGENT: &str = concat!("monochrome-tui/", env!("CARGO_PKG_VERSION"));

pub fn use_ring_for_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub(crate) fn http_client(timeout: std::time::Duration) -> reqwest::Result<reqwest::Client> {
    use_ring_for_tls();
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
}

pub fn is_transport_allowed(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
        ),
        _ => false,
    }
}

pub use auth::{AuthClient, User};
pub use catalog::{Catalog, Instance, SearchResults};
pub use error::{ApiError, ApiResult};
pub use stream::{Source, StreamConfig, StreamHandle, StreamResolver};

#[cfg(test)]
mod transport_tests {
    use super::is_transport_allowed;

    #[test]
    fn https_is_always_allowed() {
        assert!(is_transport_allowed("https://example.com/x"));
    }

    #[test]
    fn plaintext_is_refused_off_the_machine() {
        assert!(!is_transport_allowed("http://example.com"));
        assert!(!is_transport_allowed("http://10.0.0.1:80"));
        assert!(!is_transport_allowed("ftp://example.com"));
    }

    #[test]
    fn plaintext_loopback_is_allowed() {
        assert!(is_transport_allowed("http://127.0.0.1:9000"));
        assert!(is_transport_allowed("http://localhost"));
        assert!(is_transport_allowed("http://localhost:1/api"));
    }

    #[test]
    fn a_hostname_that_merely_contains_localhost_is_refused() {
        assert!(!is_transport_allowed("http://localhost.attacker.example"));
        assert!(!is_transport_allowed("http://notlocalhost"));
    }

    #[test]
    fn a_loopback_address_hidden_in_the_user_part_fools_nobody() {
        assert!(
            !is_transport_allowed("http://127.0.0.1:8080@attacker.example/"),
            "everything before the @ is a username, the request goes to attacker.example"
        );
        assert!(!is_transport_allowed("http://localhost@attacker.example/"));
        assert!(!is_transport_allowed("http://127.0.0.1@attacker.example"));
        assert!(!is_transport_allowed("http://user:pass@attacker.example/"));
    }

    #[test]
    fn a_real_host_behind_a_user_part_is_judged_on_its_own_merits() {
        assert!(is_transport_allowed(
            "http://attacker.example:80@127.0.0.1/"
        ));
        assert!(is_transport_allowed("https://user:pass@example.com/"));
    }

    #[test]
    fn loopback_over_ipv6_is_allowed_with_or_without_a_port() {
        assert!(is_transport_allowed("http://[::1]:8080/"));
        assert!(is_transport_allowed("http://[::1]/"));
    }

    #[test]
    fn something_that_is_not_a_url_at_all_is_refused() {
        assert!(!is_transport_allowed(""));
        assert!(!is_transport_allowed("   "));
        assert!(!is_transport_allowed("example.com"));
        assert!(!is_transport_allowed("http://"));
        assert!(!is_transport_allowed("javascript:alert(1)"));
    }
}
