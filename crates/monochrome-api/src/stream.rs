use crate::error::{ApiError, ApiResult};
use crate::turnstile::{self, Bridge};
use monochrome_core::model::{Quality, Track};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WEB_ORIGIN: &str = "https://monochrome.tf";
pub const DEFAULT_DEEZER_URL: &str = "https://dzr.tabs-vs-spaces.wtf";
pub const DEFAULT_PLAYBACK_URL: &str = "https://music-api.geeked.wtf";
pub const DEFAULT_PLAYBACK_TOKEN: &str = "amp_29b2lIr4mze4tK-P8QDOxfMZ9anCgJ9_uGTUks3nIyo";

pub const RETIRED_PLAYBACK_URLS: &[&str] = &[
    "https://amz.geeked.wtf",
    "https://mono.geeked.wtf",
    "https://track-api.monochrome.tf",
];

const JWT_LIFETIME: Duration = Duration::from_secs(55 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub fn is_retired_playback_url(url: &str) -> bool {
    RETIRED_PLAYBACK_URLS.contains(&url.trim().trim_end_matches('/'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Monochrome,
    Amazon,
    Tidal,
    Qobuz,
    Deezer,
    Unnamed,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Monochrome => "monochrome",
            Source::Amazon => "amazon",
            Source::Tidal => "tidal",
            Source::Qobuz => "qobuz",
            Source::Deezer => "deezer",
            Source::Unnamed => "an unnamed source",
        }
    }

    fn named(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "mono" | "monochrome" => Source::Monochrome,
            "amazon" | "amazon_music" => Source::Amazon,
            "tidal" => Source::Tidal,
            "qobuz" => Source::Qobuz,
            "deezer" => Source::Deezer,
            _ => Source::Unnamed,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamHandle {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub source: Source,
    pub quality: Option<String>,
    pub decryption_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StreamConfig {
    pub playback_enabled: bool,
    pub playback_url: String,
    pub playback_token: Option<String>,
    pub turnstile_site_key: String,
    pub turnstile_action: String,
    pub deezer_enabled: bool,
    pub deezer_url: String,
}

impl StreamConfig {
    pub fn with_defaults() -> Self {
        Self {
            playback_enabled: true,
            playback_url: DEFAULT_PLAYBACK_URL.into(),
            playback_token: None,
            turnstile_site_key: turnstile::DEFAULT_SITE_KEY.into(),
            turnstile_action: turnstile::DEFAULT_ACTION.into(),
            deezer_enabled: true,
            deezer_url: DEFAULT_DEEZER_URL.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TurnstileExchange {
    #[serde(default, alias = "jwt", alias = "token")]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PlaybackEnvelope {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default)]
    selected_source: Option<String>,
    #[serde(default)]
    quality_requested: Option<String>,
    #[serde(default)]
    playback: Vec<PlaybackResource>,
}

#[derive(Debug, Deserialize)]
struct PlaybackResource {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    delivery: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default, alias = "decryptionKey")]
    decryption_key: Option<String>,
    #[serde(default)]
    encryption: Option<serde_json::Value>,
    #[serde(default)]
    decryption: Option<serde_json::Value>,
    #[serde(default)]
    drm: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct CachedJwt {
    token: String,
    obtained: Instant,
    lifetime: Duration,
}

impl CachedJwt {
    fn is_valid(&self) -> bool {
        self.obtained.elapsed() < self.lifetime
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRecord {
    token: String,
    expires_at: u64,
}

impl SessionRecord {
    fn parse(stored: &str) -> Option<Self> {
        let trimmed = stored.trim();
        if trimmed.is_empty() {
            return None;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) if value.is_object() => {
                let token = value.get("token")?.as_str()?.to_string();
                if token.is_empty() {
                    return None;
                }
                Some(Self {
                    token,
                    expires_at: value
                        .get("expires_at")
                        .and_then(serde_json::Value::as_u64)?,
                })
            }
            _ => Some(Self {
                token: trimmed.to_string(),
                expires_at: 0,
            }),
        }
    }

    fn to_storage(&self) -> String {
        serde_json::json!({ "token": self.token, "expires_at": self.expires_at }).to_string()
    }

    fn time_left(&self, now: u64) -> Option<Duration> {
        if self.expires_at == 0 {
            return None;
        }
        self.expires_at
            .checked_sub(now)
            .filter(|left| *left > 0)
            .map(Duration::from_secs)
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

pub enum Verification {
    Ready,
    NeedsBrowser { url: String },
}

pub struct StreamResolver {
    client: reqwest::Client,
    config: StreamConfig,
    jwt: Mutex<Option<CachedJwt>>,
}

impl StreamResolver {
    pub fn new(config: StreamConfig) -> ApiResult<Self> {
        let client = crate::http_client(REQUEST_TIMEOUT)?;
        Ok(Self {
            client,
            config,
            jwt: Mutex::new(None),
        })
    }

    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    pub fn cache_jwt(&self, token: String) {
        self.cache_session(token, JWT_LIFETIME);
    }

    pub fn restore_session(&self, stored: &str) {
        let Some(record) = SessionRecord::parse(stored) else {
            return;
        };
        let Some(left) = record.time_left(unix_now()) else {
            return;
        };
        self.cache_session(record.token, left);
    }

    pub fn session_for_storage(&self) -> Option<String> {
        let guard = self.jwt.lock().expect("jwt");
        let cached = guard.as_ref().filter(|jwt| jwt.is_valid())?;
        let left = cached.lifetime.saturating_sub(cached.obtained.elapsed());
        Some(
            SessionRecord {
                token: cached.token.clone(),
                expires_at: unix_now() + left.as_secs(),
            }
            .to_storage(),
        )
    }

    fn cache_session(&self, token: String, lifetime: Duration) {
        *self.jwt.lock().expect("jwt") = Some(CachedJwt {
            token,
            obtained: Instant::now(),
            lifetime,
        });
    }

    pub fn has_session(&self) -> bool {
        self.jwt
            .lock()
            .expect("jwt")
            .as_ref()
            .is_some_and(CachedJwt::is_valid)
    }

    pub fn cached_jwt(&self) -> Option<String> {
        self.jwt
            .lock()
            .expect("jwt")
            .as_ref()
            .filter(|jwt| jwt.is_valid())
            .map(|jwt| jwt.token.clone())
    }

    fn playback_token(&self) -> &str {
        self.config
            .playback_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .unwrap_or(DEFAULT_PLAYBACK_TOKEN)
    }

    pub fn has_own_token(&self) -> bool {
        self.playback_token() != DEFAULT_PLAYBACK_TOKEN
    }

    pub fn has_playback_credential(&self) -> bool {
        self.has_own_token() || self.has_session()
    }

    pub async fn start_verification(&self) -> ApiResult<Bridge> {
        Bridge::bind(
            &self.config.turnstile_site_key,
            &self.config.turnstile_action,
        )
        .await
    }

    pub async fn finish_verification(&self, challenge_token: &str) -> ApiResult<()> {
        let base = self.config.playback_url.trim_end_matches('/');
        let response = self
            .client
            .post(format!("{base}/api/auth/turnstile"))
            .bearer_auth(self.playback_token())
            .json(&serde_json::json!({ "turnstile_token": challenge_token }))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(ApiError::Status {
                code: status.as_u16(),
                message: match gateway_message(&body) {
                    Some(message) => format!("verification was rejected: {message}"),
                    None => "verification was rejected".into(),
                },
            });
        }
        let parsed: TurnstileExchange =
            serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))?;
        let session = parsed
            .access_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| ApiError::Decode("the browser check returned no session".into()))?;
        let lifetime = parsed
            .expires_in
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or(JWT_LIFETIME);
        self.cache_session(session, lifetime);
        Ok(())
    }

    fn playback_request(&self, track: &Track, quality: Quality) -> reqwest::RequestBuilder {
        let base = self.config.playback_url.trim_end_matches('/');
        let mut request = self
            .client
            .get(format!("{base}/api/v2/track/"))
            .header("Accept", "application/json")
            .header("Origin", WEB_ORIGIN)
            .header("Referer", format!("{WEB_ORIGIN}/"))
            .bearer_auth(self.playback_token())
            .query(&[
                ("track", track.title.trim()),
                ("intent", "stream"),
                ("quality", quality.as_unified()),
            ]);

        let artist = track.artist_name().trim();
        if !artist.is_empty() {
            request = request.query(&[("artist", artist)]);
        }
        let album = track.album_title().trim();
        if !album.is_empty() {
            request = request.query(&[("album", album)]);
        }
        if let Some(isrc) = track
            .isrc
            .as_deref()
            .map(str::trim)
            .filter(|isrc| !isrc.is_empty())
        {
            request = request.query(&[("isrc", isrc.to_ascii_uppercase())]);
        }
        if track.duration > 0 {
            request = request.query(&[("duration", track.duration.to_string())]);
        }
        if let Some(session) = self.cached_jwt() {
            request = request.header("X-Turnstile-JWT", session);
        }
        request
    }

    async fn playback_body(&self, track: &Track, quality: Quality) -> ApiResult<String> {
        if track.title.trim().is_empty() {
            return Err(ApiError::Decode(
                "this track has no title to look up".into(),
            ));
        }
        if !self.has_playback_credential() {
            return Err(ApiError::TurnstileRequired);
        }

        let response = self.playback_request(track, quality).send().await?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        match status.as_u16() {
            401 => {
                self.jwt.lock().expect("jwt").take();
                if self.has_own_token() {
                    return Err(ApiError::CredentialRejected);
                }
                return Err(ApiError::TurnstileRequired);
            }
            403 => return Err(blocked_client()),
            428 => return Err(ApiError::TurnstileRequired),
            429 => {
                return Err(ApiError::Status {
                    code: 429,
                    message: "the playback service is rate limiting this client".into(),
                });
            }
            404 => return Err(ApiError::NotFound),
            code if !status.is_success() => return Err(lookup_failure(code, &body)),
            _ => {}
        }
        Ok(body)
    }

    pub async fn playback_lookup(
        &self,
        track: &Track,
        quality: Quality,
    ) -> ApiResult<serde_json::Value> {
        let body = self.playback_body(track, quality).await?;
        serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))
    }

    async fn resolve_playback(&self, track: &Track, quality: Quality) -> ApiResult<StreamHandle> {
        let body = self.playback_body(track, quality).await?;
        let envelope: PlaybackEnvelope =
            serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))?;

        if let Some(version) = envelope.schema_version.as_deref()
            && !matches!(version.split('.').next(), Some("1") | Some("2"))
        {
            return Err(ApiError::Decode(format!(
                "the playback service speaks schema {version}, this build understands 1 and 2"
            )));
        }

        let Some(resource) = envelope.playback.iter().find(|resource| playable(resource)) else {
            return Err(ApiError::NotFound);
        };

        let source = resource
            .source
            .as_deref()
            .or(envelope.selected_source.as_deref())
            .map(Source::named)
            .unwrap_or(Source::Unnamed);

        Ok(StreamHandle {
            url: resource.url.clone().unwrap_or_default(),
            headers: Vec::new(),
            source,
            quality: resource
                .quality
                .clone()
                .or_else(|| envelope.quality_requested.clone()),
            decryption_key: decryption_key(resource),
        })
    }

    pub async fn playback_health(&self) -> ApiResult<()> {
        let base = self.config.playback_url.trim_end_matches('/');
        let response = self.client.get(format!("{base}/health")).send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(ApiError::Status {
            code: status.as_u16(),
            message: gateway_message(&body).unwrap_or_else(|| "the service is unwell".into()),
        })
    }

    pub async fn gateway_client_ip(&self) -> Option<String> {
        let base = self.config.playback_url.trim_end_matches('/');
        let response = self
            .client
            .get(format!("{base}/api/track/"))
            .query(&[("title", "monochrome"), ("artist", "monochrome")])
            .send()
            .await
            .ok()?;
        let body = response.text().await.ok()?;
        let value: serde_json::Value = serde_json::from_str(&body).ok()?;
        for holder in [value.get("detail"), Some(&value)].into_iter().flatten() {
            if let Some(address) = holder.get("client_ip").and_then(serde_json::Value::as_str) {
                return Some(address.to_string());
            }
        }
        None
    }

    pub async fn validate_credential(&self) -> ApiResult<()> {
        if !self.has_playback_credential() {
            return Err(ApiError::TurnstileRequired);
        }
        let base = self.config.playback_url.trim_end_matches('/');
        let mut request = self
            .client
            .get(format!("{base}/api/v2/track/"))
            .header("Accept", "application/json")
            .bearer_auth(self.playback_token())
            .query(&[
                ("track", "monochrome"),
                ("artist", "monochrome"),
                ("intent", "stream"),
                ("quality", Quality::Lossless.as_unified()),
            ]);
        if let Some(session) = self.cached_jwt() {
            request = request.header("X-Turnstile-JWT", session);
        }

        let response = request.send().await?;
        let status = response.status();
        match status.as_u16() {
            401 => {
                self.jwt.lock().expect("jwt").take();
                Err(ApiError::CredentialRejected)
            }
            403 => Err(blocked_client()),
            428 => Err(ApiError::TurnstileRequired),
            code if code >= 500 => {
                let body = response.text().await.unwrap_or_default();
                Err(lookup_failure(code, &body))
            }
            _ => Ok(()),
        }
    }

    pub fn credential_kind(&self) -> &'static str {
        if self.has_own_token() {
            return "api token from the config";
        }
        if self
            .jwt
            .lock()
            .expect("jwt")
            .as_ref()
            .is_some_and(CachedJwt::is_valid)
        {
            return "the shared token and a playback session from the browser check";
        }
        "the shared token, no browser check yet"
    }

    pub async fn resolve(&self, track: &Track, quality: Quality) -> ApiResult<StreamHandle> {
        let mut last: Option<ApiError> = None;
        let remember = |seen: Option<ApiError>, error: ApiError| match seen {
            Some(previous) => Some(keep_the_more_useful(previous, error)),
            None => Some(error),
        };

        if self.config.playback_enabled {
            match self.resolve_playback(track, quality).await {
                Ok(handle) => return Ok(handle),
                Err(error) => last = remember(last, error),
            }
        }

        if self.config.deezer_enabled
            && let Some(isrc) = track.isrc.as_deref()
        {
            match self.resolve_deezer(isrc, quality).await {
                Ok(handle) => return Ok(handle),
                Err(error) => last = remember(last, error),
            }
        }

        Err(last.unwrap_or(ApiError::NoSourceEnabled))
    }

    async fn resolve_deezer(&self, isrc: &str, quality: Quality) -> ApiResult<StreamHandle> {
        let base = self.config.deezer_url.trim_end_matches('/');
        let url = format!(
            "{base}/stream/?isrc={}&format={}",
            urlencode(isrc),
            quality.as_deezer()
        );
        let response = self
            .client
            .head(&url)
            .header("Origin", WEB_ORIGIN)
            .header("Referer", format!("{WEB_ORIGIN}/"))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() && status.as_u16() != 405 && status.as_u16() != 501 {
            return Err(ApiError::Status {
                code: status.as_u16(),
                message: "deezer has no copy of this track".into(),
            });
        }
        Ok(StreamHandle {
            url,
            headers: vec![
                ("Origin".into(), WEB_ORIGIN.to_string()),
                ("Referer".into(), format!("{WEB_ORIGIN}/")),
            ],
            source: Source::Deezer,
            quality: Some(quality.as_deezer().to_string()),
            decryption_key: None,
        })
    }
}

fn keep_the_more_useful(primary: ApiError, fallback: ApiError) -> ApiError {
    match primary {
        ApiError::NotFound => fallback,
        primary => primary,
    }
}

fn lookup_failure(code: u16, body: &str) -> ApiError {
    ApiError::Status {
        code,
        message: match gateway_message(body) {
            Some(message) => format!("playback lookup failed: {message}"),
            None => "playback lookup failed".into(),
        },
    }
}

fn blocked_client() -> ApiError {
    ApiError::Status {
        code: 403,
        message: "the playback service has blocked this address for now".into(),
    }
}

fn playable(resource: &PlaybackResource) -> bool {
    let Some(url) = resource.url.as_deref() else {
        return false;
    };
    url.starts_with("https://")
        && matches!(resource.delivery.as_deref(), None | Some("direct"))
        && matches!(resource.kind.as_deref(), Some("audio") | Some("manifest"))
        && !url.contains(".mpd")
        && !url.contains(".m3u8")
}

fn decryption_key(resource: &PlaybackResource) -> Option<String> {
    if let Some(key) = resource
        .decryption_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        return Some(key.to_string());
    }
    [&resource.encryption, &resource.decryption, &resource.drm]
        .into_iter()
        .flatten()
        .find_map(nested_key)
}

fn nested_key(holder: &serde_json::Value) -> Option<String> {
    for name in ["key", "decryption_key", "decryptionKey"] {
        let Some(found) = holder.get(name) else {
            continue;
        };
        let text = match found {
            serde_json::Value::String(text) => Some(text.as_str()),
            other => other.get("value").and_then(serde_json::Value::as_str),
        };
        if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
            return Some(text.to_string());
        }
    }
    None
}

fn gateway_message(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        for key in ["detail", "message", "error"] {
            if let Some(text) = value.get(key).and_then(serde_json::Value::as_str)
                && !text.trim().is_empty()
            {
                return Some(text.trim().to_string());
            }
        }
        return None;
    }
    let readable = if trimmed.starts_with('<') {
        tagged_text(trimmed, "title")
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| strip_tags(trimmed))
    } else {
        trimmed.to_string()
    };
    let flattened: String = readable
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = flattened
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect::<String>();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn tagged_text(body: &str, tag: &str) -> Option<String> {
    let lowered = body.to_ascii_lowercase();
    let open = lowered.find(&format!("<{tag}"))?;
    let start = open + lowered[open..].find('>')? + 1;
    let close = lowered[start..].find(&format!("</{tag}"))? + start;
    Some(body[start..close].to_string())
}

fn strip_tags(body: &str) -> String {
    let mut text = String::new();
    let mut rest = body;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        text.push(' ');
        let tail = &rest[start..];
        let closer = if tail.starts_with("<!--") { "-->" } else { ">" };
        match tail.find(closer) {
            Some(offset) => rest = &tail[offset + closer.len()..],
            None => return text,
        }
    }
    text.push_str(rest);
    text
}

fn urlencode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use monochrome_core::model::ArtistRef;

    #[test]
    fn a_stored_session_keeps_only_the_time_it_has_left() {
        let record = SessionRecord {
            token: "abc".into(),
            expires_at: 1_000,
        };
        assert_eq!(record.time_left(400), Some(Duration::from_secs(600)));
        assert_eq!(record.time_left(1_000), None, "expired is expired");
        assert_eq!(record.time_left(5_000), None);
    }

    #[test]
    fn a_stored_session_survives_a_round_trip() {
        let record = SessionRecord {
            token: "abc".into(),
            expires_at: 1_700_000_000,
        };
        assert_eq!(SessionRecord::parse(&record.to_storage()), Some(record));
    }

    #[test]
    fn a_session_stored_without_an_expiry_is_not_trusted() {
        let legacy = SessionRecord::parse("a-bare-token").expect("parsed");
        assert_eq!(legacy.expires_at, 0);
        assert_eq!(
            legacy.time_left(1_000),
            None,
            "an unknown age cannot be assumed fresh"
        );
    }

    #[test]
    fn nonsense_in_the_keyring_is_ignored_rather_than_trusted() {
        assert_eq!(SessionRecord::parse(""), None);
        assert_eq!(SessionRecord::parse("   "), None);
        assert_eq!(SessionRecord::parse(r#"{"token":""}"#), None);
        assert_eq!(SessionRecord::parse(r#"{"expires_at":5}"#), None);
    }

    #[test]
    fn an_expired_stored_session_is_not_restored() {
        let resolver = resolver(StreamConfig::with_defaults());
        let stale = SessionRecord {
            token: "old".into(),
            expires_at: 1,
        };
        resolver.restore_session(&stale.to_storage());
        assert!(!resolver.has_session());
    }

    #[test]
    fn a_live_stored_session_is_restored_with_its_remaining_time() {
        let resolver = resolver(StreamConfig::with_defaults());
        let fresh = SessionRecord {
            token: "good".into(),
            expires_at: unix_now() + 900,
        };
        resolver.restore_session(&fresh.to_storage());
        assert!(resolver.has_session());
        assert_eq!(resolver.cached_jwt().as_deref(), Some("good"));

        let again = resolver.session_for_storage().expect("stored again");
        let parsed = SessionRecord::parse(&again).expect("parsed");
        assert!(
            parsed.expires_at <= fresh.expires_at,
            "storing must not extend a session"
        );
    }

    #[test]
    fn the_gateways_own_words_survive_into_the_error() {
        let error = lookup_failure(
            500,
            r#"{"detail":"[Amazon-Direct] Manifest request failed: 400"}"#,
        );
        assert_eq!(
            error.to_string(),
            "server returned 500: playback lookup failed: [Amazon-Direct] Manifest request failed: 400"
        );
    }

    #[test]
    fn a_body_that_says_nothing_still_reports_the_code() {
        assert_eq!(
            lookup_failure(502, "").to_string(),
            "server returned 502: playback lookup failed"
        );
        assert_eq!(
            lookup_failure(502, "   ").to_string(),
            "server returned 502: playback lookup failed"
        );
    }

    #[test]
    fn an_html_error_page_is_flattened_rather_than_dumped() {
        let message = gateway_message("<html>\n  <body>Bad Gateway</body>\n</html>")
            .expect("something to show");
        assert!(!message.contains('\n'));
        assert!(message.len() <= 160);
        assert!(message.contains("Bad Gateway"));
    }

    #[test]
    fn a_full_error_page_reports_its_title_rather_than_its_boilerplate() {
        let page = concat!(
            "<!DOCTYPE html>\n",
            "<!--[if lt IE 7]> <html class=\"no-js ie6 oldie\" lang=\"en-US\"> <![endif]-->\n",
            "<html lang=\"en\">\n<head>\n",
            "<meta charset=\"UTF-8\">\n",
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n",
            "<title>track-api.monochrome.tf | 502: Bad gateway</title>\n",
            "</head>\n<body>\nThe web server reported a bad gateway error.\n</body>\n</html>"
        );
        let message = gateway_message(page).expect("something to show");
        assert_eq!(message, "track-api.monochrome.tf | 502: Bad gateway");
        assert!(!message.contains("DOCTYPE"), "{message}");
        assert!(!message.contains('<'), "{message}");
    }

    #[test]
    fn a_suspended_service_page_says_it_was_suspended() {
        let page = "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<title>Service Suspended</title>\n</head>\n<body>\nThis service has been suspended by its owner.\n</body>\n</html>";
        assert_eq!(
            gateway_message(page).expect("something to show"),
            "Service Suspended"
        );
    }

    #[test]
    fn the_fallbacks_complaint_never_hides_why_the_main_source_failed() {
        let primary = ApiError::Status {
            code: 500,
            message: "amazon lookup failed: upstream is down".into(),
        };
        let fallback = ApiError::Status {
            code: 503,
            message: "deezer has no copy of this track".into(),
        };
        let kept = keep_the_more_useful(primary, fallback);
        assert!(kept.to_string().contains("upstream is down"), "got: {kept}");
    }

    #[test]
    fn verification_still_outranks_everything_the_fallback_says() {
        let kept = keep_the_more_useful(
            ApiError::TurnstileRequired,
            ApiError::Status {
                code: 503,
                message: "deezer has no copy of this track".into(),
            },
        );
        assert!(matches!(kept, ApiError::TurnstileRequired));
    }

    #[test]
    fn a_source_that_simply_had_nothing_defers_to_the_next_one() {
        let kept = keep_the_more_useful(
            ApiError::NotFound,
            ApiError::Status {
                code: 503,
                message: "deezer has no copy of this track".into(),
            },
        );
        assert!(kept.to_string().contains("deezer"), "got: {kept}");
    }

    #[tokio::test]
    async fn switching_every_source_off_says_so_plainly() {
        let mut config = StreamConfig::with_defaults();
        config.playback_enabled = false;
        config.deezer_enabled = false;
        let error = resolver(config)
            .resolve(&track(), Quality::Lossless)
            .await
            .expect_err("nothing can play");
        assert!(matches!(error, ApiError::NoSourceEnabled), "got: {error}");
    }

    fn track() -> Track {
        Track {
            id: 1,
            title: "One More Time".into(),
            duration: 320,
            explicit: false,
            artist: Some(ArtistRef {
                id: 8847,
                name: "Daft Punk".into(),
                picture: None,
            }),
            artists: Vec::new(),
            album: None,
            isrc: Some("GBDUW0000053".into()),
            track_number: Some(1),
            volume_number: Some(1),
            copyright: None,
            version: None,
            quality: Quality::Lossless,
            replay_gain: None,
            peak: None,
            stream_ready: true,
        }
    }

    fn resolver(config: StreamConfig) -> StreamResolver {
        StreamResolver::new(config).expect("resolver")
    }

    #[test]
    fn without_a_token_of_your_own_the_shared_one_is_used() {
        let resolver = resolver(StreamConfig::with_defaults());
        assert_eq!(resolver.playback_token(), DEFAULT_PLAYBACK_TOKEN);
        assert!(!resolver.has_own_token());
    }

    #[test]
    fn a_token_from_the_config_replaces_the_shared_one() {
        let mut config = StreamConfig::with_defaults();
        config.playback_token = Some("  mine  ".into());
        let resolver = resolver(config);
        assert_eq!(resolver.playback_token(), "mine");
        assert!(resolver.has_own_token());
        assert!(resolver.has_playback_credential());
    }

    #[test]
    fn a_blank_token_counts_as_absent() {
        let mut config = StreamConfig::with_defaults();
        config.playback_token = Some("   ".into());
        let resolver = resolver(config);
        assert!(!resolver.has_own_token());
        assert!(!resolver.has_playback_credential());
    }

    #[test]
    fn the_shared_token_needs_a_browser_check_before_it_counts() {
        let resolver = resolver(StreamConfig::with_defaults());
        assert!(!resolver.has_playback_credential());
        resolver.cache_jwt("jwt-value".into());
        assert!(resolver.has_playback_credential());
    }

    #[test]
    fn every_url_the_service_left_behind_is_recognised() {
        assert!(is_retired_playback_url("https://track-api.monochrome.tf"));
        assert!(is_retired_playback_url("https://amz.geeked.wtf/"));
        assert!(is_retired_playback_url("  https://mono.geeked.wtf  "));
        assert!(!is_retired_playback_url(DEFAULT_PLAYBACK_URL));
        assert!(!is_retired_playback_url("https://my-own-mirror.example"));
    }

    #[test]
    fn only_a_resource_this_player_can_read_is_offered() {
        let direct = resource(r#"{"url":"https://cdn.example/a.flac","kind":"audio"}"#);
        assert!(playable(&direct));

        let dash =
            resource(r#"{"url":"https://cdn.example/a.mpd","kind":"manifest","delivery":"dash"}"#);
        assert!(!playable(&dash), "this build cannot read a dash manifest");

        let hls =
            resource(r#"{"url":"https://cdn.example/a.m3u8","kind":"audio","delivery":"hls"}"#);
        assert!(!playable(&hls));

        let plaintext = resource(r#"{"url":"http://cdn.example/a.flac","kind":"audio"}"#);
        assert!(!playable(&plaintext), "an unencrypted hop is refused");

        let nameless = resource(r#"{"url":"https://cdn.example/a.flac"}"#);
        assert!(!playable(&nameless), "an unlabelled resource is skipped");
    }

    #[test]
    fn a_key_is_found_wherever_the_service_chooses_to_put_it() {
        let flat = resource(r#"{"decryption_key":"aa"}"#);
        assert_eq!(decryption_key(&flat).as_deref(), Some("aa"));

        let nested = resource(r#"{"encryption":{"key":{"value":"bb"}}}"#);
        assert_eq!(decryption_key(&nested).as_deref(), Some("bb"));

        let plain = resource(r#"{"decryption":{"key":"cc"}}"#);
        assert_eq!(decryption_key(&plain).as_deref(), Some("cc"));

        let drm = resource(r#"{"drm":{"decryption_key":"dd"}}"#);
        assert_eq!(decryption_key(&drm).as_deref(), Some("dd"));

        let none = resource(r#"{"encryption":{"key":{"value":"  "}}}"#);
        assert_eq!(decryption_key(&none), None);
    }

    #[test]
    fn every_source_the_service_can_pick_has_a_name() {
        assert_eq!(Source::named("mono").label(), "monochrome");
        assert_eq!(Source::named("Amazon").label(), "amazon");
        assert_eq!(Source::named(" tidal ").label(), "tidal");
        assert_eq!(Source::named("qobuz").label(), "qobuz");
        assert_eq!(Source::named("something-new"), Source::Unnamed);
    }

    fn resource(json: &str) -> PlaybackResource {
        serde_json::from_str(json).expect("resource")
    }

    #[test]
    fn a_cached_jwt_can_be_read_back_for_storage() {
        let resolver = resolver(StreamConfig::with_defaults());
        assert_eq!(resolver.cached_jwt(), None);
        resolver.cache_jwt("jwt-value".into());
        assert_eq!(resolver.cached_jwt().as_deref(), Some("jwt-value"));
    }

    #[test]
    fn an_expired_jwt_is_not_offered_for_storage() {
        let resolver = resolver(StreamConfig::with_defaults());
        *resolver.jwt.lock().unwrap() = Some(CachedJwt {
            token: "old".into(),
            obtained: Instant::now() - JWT_LIFETIME - Duration::from_secs(1),
            lifetime: JWT_LIFETIME,
        });
        assert_eq!(resolver.cached_jwt(), None);
    }

    #[test]
    fn an_expired_jwt_is_ignored() {
        let resolver = resolver(StreamConfig::with_defaults());
        *resolver.jwt.lock().unwrap() = Some(CachedJwt {
            token: "old".into(),
            obtained: Instant::now() - JWT_LIFETIME - Duration::from_secs(1),
            lifetime: JWT_LIFETIME,
        });
        assert!(!resolver.has_playback_credential());
    }

    #[test]
    fn quality_tokens_match_the_web_client() {
        assert_eq!(Quality::HiRes.as_unified(), "HI_RES_LOSSLESS");
        assert_eq!(Quality::Lossless.as_unified(), "LOSSLESS");
        assert_eq!(Quality::High.as_unified(), "HIGH");
        assert_eq!(Quality::Low.as_unified(), "LOW");
        assert_eq!(Quality::Atmos.as_unified(), "DOLBY_ATMOS");
    }

    #[tokio::test]
    async fn a_track_without_an_isrc_never_reaches_deezer() {
        let mut config = StreamConfig::with_defaults();
        config.playback_enabled = false;
        config.deezer_enabled = true;
        let resolver = resolver(config);
        let mut bare = track();
        bare.isrc = None;
        let error = resolver.resolve(&bare, Quality::Lossless).await;
        assert!(matches!(error, Err(ApiError::NoSourceEnabled)));
    }

    #[tokio::test]
    async fn playback_without_a_credential_reports_that_verification_is_needed() {
        let mut config = StreamConfig::with_defaults();
        config.deezer_enabled = false;
        let resolver = resolver(config);
        let error = resolver.resolve(&track(), Quality::Lossless).await;
        assert!(matches!(error, Err(ApiError::TurnstileRequired)));
    }
}
