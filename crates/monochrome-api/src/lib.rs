pub mod auth;
pub mod cache;
pub mod catalog;
pub mod error;
pub mod jwt;
pub mod stream;
pub mod turnstile;
pub mod wire;

pub const USER_AGENT: &str = concat!("monochrome-tui/", env!("CARGO_PKG_VERSION"));

pub fn is_transport_allowed(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(rest.split(['/', '?', '#']).next().unwrap_or_default());
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
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
        assert!(!is_transport_allowed("http://localhost.evil.com"));
        assert!(!is_transport_allowed("http://notlocalhost"));
    }
}
