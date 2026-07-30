use crate::error::{ApiError, ApiResult};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const DEFAULT_SITE_KEY: &str = "0x4AAAAAADgxqF6QVMm0GLHH";
const SOLVE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REQUEST_BYTES: usize = 16 * 1024;

pub struct Bridge {
    listener: TcpListener,
    nonce: String,
    site_key: String,
}

impl Bridge {
    pub async fn bind(site_key: &str) -> ApiResult<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| ApiError::Network(format!("cannot open the local bridge: {error}")))?;
        Ok(Self {
            listener,
            nonce: nonce(),
            site_key: site_key.to_string(),
        })
    }

    pub fn url(&self) -> String {
        let port = self
            .listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or_default();
        format!("http://localhost:{port}/?n={}", self.nonce)
    }

    pub async fn wait_for_token(self) -> ApiResult<String> {
        tokio::time::timeout(SOLVE_TIMEOUT, self.accept_loop())
            .await
            .map_err(|_| ApiError::Network("verification timed out".into()))?
    }

    async fn accept_loop(self) -> ApiResult<String> {
        loop {
            let (mut stream, address) = self
                .listener
                .accept()
                .await
                .map_err(|error| ApiError::Network(error.to_string()))?;

            if !address.ip().is_loopback() {
                continue;
            }

            let Some(request) = read_request(&mut stream).await else {
                continue;
            };

            let Some(target) = request_target(&request) else {
                respond(&mut stream, "400 Bad Request", "text/plain", "bad request").await;
                continue;
            };

            let (path, query) = split_target(&target);
            let supplied_nonce = param(query, "n").unwrap_or_default();

            if supplied_nonce != self.nonce {
                respond(&mut stream, "403 Forbidden", "text/plain", "forbidden").await;
                continue;
            }

            match path {
                "/" => {
                    let page = challenge_page(&self.site_key, &self.nonce);
                    respond(&mut stream, "200 OK", "text/html; charset=utf-8", &page).await;
                }
                "/token" => {
                    if let Some(code) = param(query, "e").filter(|code| !code.is_empty()) {
                        respond(&mut stream, "200 OK", "text/plain", "reported").await;
                        return Err(ApiError::Status {
                            code: 0,
                            message: describe_turnstile_error(&code),
                        });
                    }
                    let token = param(query, "t").unwrap_or_default();
                    if token.is_empty() {
                        respond(&mut stream, "400 Bad Request", "text/plain", "no token").await;
                        continue;
                    }
                    respond(&mut stream, "200 OK", "text/html; charset=utf-8", DONE_PAGE).await;
                    return Ok(token);
                }
                _ => {
                    respond(&mut stream, "404 Not Found", "text/plain", "not found").await;
                }
            }
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = vec![0u8; MAX_REQUEST_BYTES];
    let mut filled = 0;
    loop {
        let read = stream.read(&mut buffer[filled..]).await.ok()?;
        if read == 0 {
            break;
        }
        filled += read;
        if buffer[..filled].windows(4).any(|w| w == b"\r\n\r\n") || filled == buffer.len() {
            break;
        }
    }
    Some(String::from_utf8_lossy(&buffer[..filled]).into_owned())
}

fn request_target(request: &str) -> Option<String> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    if method != "GET" {
        return None;
    }
    Some(parts.next()?.to_string())
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    }
}

fn param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

fn nonce() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..32)
        .map(|_| {
            let index: usize = rng.random_range(0..16);
            char::from_digit(index as u32, 16).unwrap_or('0')
        })
        .collect()
}

pub fn describe_turnstile_error(code: &str) -> String {
    let explanation = match code {
        code if code.starts_with("1102") => {
            "this gateway's Turnstile key does not accept a local address"
        }
        code if code.starts_with("1060") => "Cloudflare could not be reached",
        code if code.starts_with("3000") || code.starts_with("6000") => {
            "the challenge was rejected"
        }
        _ => "the browser check did not complete",
    };
    format!("turnstile {code}: {explanation}")
}

fn challenge_page(site_key: &str, nonce: &str) -> String {
    format!(
        r#"<!doctype html>
<meta charset="utf-8">
<title>Monochrome</title>
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js?onload=start" async defer></script>
<style>
body {{ font: 14px system-ui, sans-serif; display: grid; place-items: center; min-height: 90vh; margin: 0; }}
main {{ text-align: center; }}
p {{ color: #666; }}
</style>
<main>
<h1>Monochrome</h1>
<p id="status">Verifying your browser, this closes itself.</p>
<div id="widget"></div>
<p id="detail"></p>
</main>
<script>
function start() {{
  turnstile.render('#widget', {{
    sitekey: '{site_key}',
    callback: function (token) {{
      fetch('/token?n={nonce}&t=' + encodeURIComponent(token))
        .then(function () {{
          document.getElementById('status').textContent = 'Done. You can close this tab.';
          window.close();
        }});
    }},
    'error-callback': function (code) {{
      document.getElementById('status').textContent = 'Verification failed (' + code + ').';
      document.getElementById('detail').textContent =
        'Return to the terminal, it will tell you what to do next.';
      fetch('/token?n={nonce}&e=' + encodeURIComponent(code || 'unknown'));
    }},
  }});
}}
</script>
"#
    )
}

const DONE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Monochrome</title>\
<p style=\"font:14px system-ui,sans-serif\">Verified. You can close this tab.</p>\
<script>window.close()</script>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_are_long_and_unpredictable() {
        let first = nonce();
        let second = nonce();
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
    }

    #[test]
    fn query_parameters_are_decoded() {
        assert_eq!(param("n=abc&t=x%2By", "t"), Some("x+y".into()));
        assert_eq!(param("n=abc", "t"), None);
    }

    #[test]
    fn targets_split_into_path_and_query() {
        assert_eq!(split_target("/token?n=1&t=2"), ("/token", "n=1&t=2"));
        assert_eq!(split_target("/"), ("/", ""));
    }

    #[test]
    fn only_get_requests_are_accepted() {
        assert!(request_target("POST /token HTTP/1.1\r\n").is_none());
        assert_eq!(
            request_target("GET /token?a=b HTTP/1.1\r\n").as_deref(),
            Some("/token?a=b")
        );
    }

    #[test]
    fn a_domain_error_is_explained_in_plain_words() {
        let message = describe_turnstile_error("110200");
        assert!(message.contains("110200"));
        assert!(message.contains("local address"));
    }

    #[test]
    fn an_unknown_error_code_still_produces_a_message() {
        assert!(describe_turnstile_error("999999").contains("999999"));
    }

    #[tokio::test]
    async fn a_reported_challenge_error_reaches_the_caller() {
        let bridge = Bridge::bind(DEFAULT_SITE_KEY).await.expect("bridge");
        let port = bridge.listener.local_addr().unwrap().port();
        let url = bridge.url();
        let nonce = url.rsplit("n=").next().unwrap().to_string();
        let task = tokio::spawn(bridge.wait_for_token());

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let request = format!("GET /token?n={nonce}&e=110200 HTTP/1.1\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response).await;

        let error = task
            .await
            .expect("join")
            .expect_err("should report the failure");
        assert!(error.to_string().contains("110200"), "{error}");
    }

    #[test]
    fn the_challenge_page_carries_the_site_key_and_nonce() {
        let page = challenge_page("0xSITEKEY", "abcd");
        assert!(page.contains("0xSITEKEY"));
        assert!(page.contains("/token?n=abcd"));
    }

    #[tokio::test]
    async fn the_bridge_binds_to_loopback_only() {
        let bridge = Bridge::bind(DEFAULT_SITE_KEY).await.expect("bridge");
        let address = bridge.listener.local_addr().expect("addr");
        assert!(address.ip().is_loopback());
        assert!(bridge.url().starts_with("http://localhost:"));
    }

    #[tokio::test]
    async fn a_wrong_nonce_is_refused() {
        let bridge = Bridge::bind(DEFAULT_SITE_KEY).await.expect("bridge");
        let port = bridge.listener.local_addr().unwrap().port();
        let task = tokio::spawn(bridge.wait_for_token());

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        stream
            .write_all(b"GET /token?n=wrong&t=abc HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write");
        let mut response = String::new();
        stream.read_to_string(&mut response).await.expect("read");
        assert!(response.contains("403 Forbidden"));
        task.abort();
    }

    #[tokio::test]
    async fn a_valid_callback_yields_the_token() {
        let bridge = Bridge::bind(DEFAULT_SITE_KEY).await.expect("bridge");
        let port = bridge.listener.local_addr().unwrap().port();
        let url = bridge.url();
        let nonce = url.rsplit("n=").next().unwrap().to_string();
        let task = tokio::spawn(bridge.wait_for_token());

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let request =
            format!("GET /token?n={nonce}&t=cf-token-value HTTP/1.1\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response).await;

        let token = task.await.expect("join").expect("token");
        assert_eq!(token, "cf-token-value");
    }
}
