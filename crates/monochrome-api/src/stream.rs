use crate::error::{ApiError, ApiResult};
use crate::turnstile::{self, Bridge};
use monochrome_core::model::{Quality, Track};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_AMAZON_URL: &str = "https://amz.geeked.wtf";
const WEB_ORIGIN: &str = "https://monochrome.tf";
pub const DEFAULT_DEEZER_URL: &str = "https://dzr.tabs-vs-spaces.wtf";
pub const DEFAULT_PLAYBACK_URL: &str = "https://track-api.monochrome.tf";
const JWT_LIFETIME: Duration = Duration::from_secs(55 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Monochrome,
    Amazon,
    Deezer,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Monochrome => "monochrome",
            Source::Amazon => "amazon",
            Source::Deezer => "deezer",
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
    pub amazon_enabled: bool,
    pub amazon_url: String,
    pub amazon_bypass_token: Option<String>,
    pub amazon_api_key: Option<String>,
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
            amazon_enabled: true,
            amazon_url: DEFAULT_AMAZON_URL.into(),
            amazon_bypass_token: None,
            amazon_api_key: None,
            turnstile_site_key: turnstile::DEFAULT_SITE_KEY.into(),
            turnstile_action: turnstile::DEFAULT_ACTION.into(),
            deezer_enabled: true,
            deezer_url: DEFAULT_DEEZER_URL.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AmazonTrack {
    #[serde(default)]
    asin: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    quality_selected: Option<String>,
    #[serde(default)]
    stream_url: Option<String>,
    #[serde(default)]
    decryption_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TurnstileExchange {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PlaybackAnswer {
    url: String,
    #[serde(default)]
    title: Option<String>,
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

type Credential = (Vec<(String, String)>, Vec<(String, String)>);

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

    pub fn has_static_amazon_credential(&self) -> bool {
        self.config
            .amazon_bypass_token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
            || self
                .config
                .amazon_api_key
                .as_ref()
                .is_some_and(|k| !k.is_empty())
    }

    pub fn has_amazon_credential(&self) -> bool {
        self.has_static_amazon_credential() || self.has_session()
    }

    fn amazon_credential(&self) -> Option<Credential> {
        if let Some(token) = self
            .config
            .amazon_bypass_token
            .as_ref()
            .filter(|t| !t.is_empty())
        {
            return Some((vec![("bypass_token".into(), token.clone())], Vec::new()));
        }
        if let Some(key) = self
            .config
            .amazon_api_key
            .as_ref()
            .filter(|k| !k.is_empty())
        {
            return Some((Vec::new(), vec![("X-API-Key".into(), key.clone())]));
        }
        let guard = self.jwt.lock().expect("jwt");
        let cached = guard.as_ref().filter(|jwt| jwt.is_valid())?;
        Some((
            Vec::new(),
            vec![("X-Turnstile-JWT".into(), cached.token.clone())],
        ))
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
            .post(format!("{base}/auth/turnstile"))
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
        let lifetime = parsed
            .expires_in
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or(JWT_LIFETIME);
        self.cache_session(parsed.access_token, lifetime);
        Ok(())
    }

    async fn resolve_monochrome(&self, track: &Track) -> ApiResult<StreamHandle> {
        let Some(session) = self.cached_jwt() else {
            return Err(ApiError::TurnstileRequired);
        };
        let base = self.config.playback_url.trim_end_matches('/');

        let mut body = serde_json::json!({
            "song_name": track.title,
            "artist": track.artist_name(),
        });
        if let Some(isrc) = track.isrc.as_deref().filter(|isrc| !isrc.is_empty()) {
            body["isrc"] = serde_json::Value::from(isrc);
        }
        if track.duration > 0 {
            body["duration"] = serde_json::Value::from(track.duration);
        }

        let response = self
            .client
            .post(format!("{base}/playback"))
            .bearer_auth(&session)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        match status.as_u16() {
            401 | 403 => {
                self.jwt.lock().expect("jwt").take();
                return Err(ApiError::TurnstileRequired);
            }
            429 => {
                return Err(ApiError::Status {
                    code: 429,
                    message: "the playback service is rate limiting this client".into(),
                });
            }
            code if !status.is_success() => {
                return Err(ApiError::Status {
                    code,
                    message: match gateway_message(&text) {
                        Some(message) => format!("playback lookup failed: {message}"),
                        None => "playback lookup failed".into(),
                    },
                });
            }
            _ => {}
        }

        let answer: PlaybackAnswer =
            serde_json::from_str(&text).map_err(|error| ApiError::Decode(error.to_string()))?;
        if !answer.url.starts_with("https://") {
            return Err(ApiError::Decode(
                "the playback service returned no usable address".into(),
            ));
        }
        let _ = answer.title;

        Ok(StreamHandle {
            url: answer.url,
            headers: Vec::new(),
            source: Source::Monochrome,
            quality: Some("LOSSLESS".into()),
            decryption_key: None,
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
        let base = self.config.amazon_url.trim_end_matches('/');
        let response = self
            .client
            .get(format!("{base}/api/track/"))
            .query(&[("title", "monochrome"), ("artist", "monochrome")])
            .send()
            .await
            .ok()?;
        let body = response.text().await.ok()?;
        let value: serde_json::Value = serde_json::from_str(&body).ok()?;
        value
            .get("client_ip")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }

    pub async fn validate_credential(&self) -> ApiResult<()> {
        let Some((query, headers)) = self.amazon_credential() else {
            return Err(ApiError::TurnstileRequired);
        };
        let base = self.config.amazon_url.trim_end_matches('/');
        let mut request = self.client.get(format!("{base}/api/track/")).query(&[
            ("title", "monochrome"),
            ("artist", "monochrome"),
            ("quality", "HD"),
        ]);
        for (key, value) in &query {
            request = request.query(&[(key.as_str(), value.as_str())]);
        }
        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await?;
        let status = response.status();
        match status.as_u16() {
            401 => {
                self.jwt.lock().expect("jwt").take();
                Err(ApiError::CredentialRejected)
            }
            428 => Err(ApiError::TurnstileRequired),
            code if code >= 500 => {
                let body = response.text().await.unwrap_or_default();
                Err(lookup_failure(code, &body))
            }
            _ => Ok(()),
        }
    }

    pub fn credential_kind(&self) -> &'static str {
        if self
            .config
            .amazon_bypass_token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
        {
            return "bypass token from the config";
        }
        if self
            .config
            .amazon_api_key
            .as_ref()
            .is_some_and(|k| !k.is_empty())
        {
            return "api key from the config";
        }
        if self
            .jwt
            .lock()
            .expect("jwt")
            .as_ref()
            .is_some_and(CachedJwt::is_valid)
        {
            return "playback session from the browser check";
        }
        "none"
    }

    pub async fn amazon_lookup(
        &self,
        track: &Track,
        quality: Quality,
    ) -> ApiResult<serde_json::Value> {
        let Some((query, headers)) = self.amazon_credential() else {
            return Err(ApiError::TurnstileRequired);
        };
        let base = self.config.amazon_url.trim_end_matches('/');
        let mut request = self
            .client
            .get(format!("{base}/api/track/"))
            .header("Origin", WEB_ORIGIN)
            .header("Referer", format!("{WEB_ORIGIN}/"))
            .query(&[
                ("title", track.title.as_str()),
                ("artist", track.artist_name()),
                ("album", track.album_title()),
                ("quality", quality.as_amazon()),
            ]);
        if let Some(isrc) = track.isrc.as_deref() {
            request = request.query(&[("isrc", isrc)]);
        }
        for (key, value) in &query {
            request = request.query(&[(key.as_str(), value.as_str())]);
        }
        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if status.as_u16() == 401 {
            return Err(ApiError::CredentialRejected);
        }
        if status.as_u16() == 428 {
            return Err(ApiError::TurnstileRequired);
        }
        if !status.is_success() {
            return Err(lookup_failure(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))
    }

    pub async fn resolve(&self, track: &Track, quality: Quality) -> ApiResult<StreamHandle> {
        let mut last = ApiError::Network("no source is enabled".into());

        if self.config.playback_enabled {
            match self.resolve_monochrome(track).await {
                Ok(handle) => return Ok(handle),
                Err(error) => last = error,
            }
        }

        if self.config.amazon_enabled && self.has_static_amazon_credential() {
            match self.resolve_amazon(track, quality).await {
                Ok(handle) => return Ok(handle),
                Err(error) => last = keep_the_more_useful(last, error),
            }
        }

        if self.config.deezer_enabled
            && let Some(isrc) = track.isrc.as_deref()
        {
            match self.resolve_deezer(isrc, quality).await {
                Ok(handle) => return Ok(handle),
                Err(error) => last = keep_the_more_useful(last, error),
            }
        }

        Err(last)
    }

    async fn resolve_amazon(&self, track: &Track, quality: Quality) -> ApiResult<StreamHandle> {
        let Some((query, headers)) = self.amazon_credential() else {
            return Err(ApiError::TurnstileRequired);
        };
        let base = self.config.amazon_url.trim_end_matches('/');

        let mut request = self.client.get(format!("{base}/api/track/")).query(&[
            ("title", track.title.as_str()),
            ("artist", track.artist_name()),
            ("album", track.album_title()),
            ("quality", quality.as_amazon()),
        ]);
        if let Some(isrc) = track.isrc.as_deref() {
            request = request.query(&[("isrc", isrc)]);
        }
        for (key, value) in &query {
            request = request.query(&[(key.as_str(), value.as_str())]);
        }
        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await?;
        let status = response.status();
        if status.as_u16() == 401 {
            self.jwt.lock().expect("jwt").take();
            return Err(ApiError::CredentialRejected);
        }
        if status.as_u16() == 428 {
            return Err(ApiError::TurnstileRequired);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(lookup_failure(status.as_u16(), &body));
        }

        let body = response.text().await?;
        let payload = extract_track_payload(&body)?;

        let direct = payload
            .stream_url
            .as_deref()
            .filter(|url| url.starts_with("https://"))
            .map(str::to_string);

        let Some(url) = direct else {
            return Err(ApiError::Decode(
                "amazon returned no stream address for this track".into(),
            ));
        };

        Ok(StreamHandle {
            url,
            headers: Vec::new(),
            source: Source::Amazon,
            quality: payload.quality_selected,
            decryption_key: payload.decryption_key,
        })
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
    match (&primary, &fallback) {
        (ApiError::TurnstileRequired | ApiError::CredentialRejected, _) => primary,
        (ApiError::Network(reason), _) if reason == "no source is enabled" => fallback,
        (ApiError::Status { .. } | ApiError::Decode(_) | ApiError::Network(_), _) => primary,
        _ => fallback,
    }
}

fn lookup_failure(code: u16, body: &str) -> ApiError {
    ApiError::Status {
        code,
        message: match gateway_message(body) {
            Some(message) => format!("amazon lookup failed: {message}"),
            None => "amazon lookup failed".into(),
        },
    }
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
    let flattened: String = trimmed
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(160)
        .collect();
    let cleaned = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

fn extract_track_payload(body: &str) -> ApiResult<AmazonTrack> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| ApiError::Decode(error.to_string()))?;
    for candidate in [
        value.get("data"),
        value.get("track"),
        value.get("result"),
        Some(&value),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(parsed) = serde_json::from_value::<AmazonTrack>(candidate.clone())
            && (parsed.asin.is_some() || parsed.id.is_some())
        {
            return Ok(parsed);
        }
    }
    Err(ApiError::Decode(
        "amazon returned an unexpected body".into(),
    ))
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
            "server returned 500: amazon lookup failed: [Amazon-Direct] Manifest request failed: 400"
        );
    }

    #[test]
    fn a_body_that_says_nothing_still_reports_the_code() {
        assert_eq!(
            lookup_failure(502, "").to_string(),
            "server returned 502: amazon lookup failed"
        );
        assert_eq!(
            lookup_failure(502, "   ").to_string(),
            "server returned 502: amazon lookup failed"
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
    fn with_no_source_tried_the_fallback_reason_is_the_only_one_there_is() {
        let kept = keep_the_more_useful(
            ApiError::Network("no source is enabled".into()),
            ApiError::Status {
                code: 503,
                message: "deezer has no copy of this track".into(),
            },
        );
        assert!(kept.to_string().contains("deezer"));
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
    fn a_bypass_token_travels_as_a_query_parameter() {
        let mut config = StreamConfig::with_defaults();
        config.amazon_bypass_token = Some("secret".into());
        let (query, headers) = resolver(config).amazon_credential().expect("credential");
        assert_eq!(
            query,
            vec![("bypass_token".to_string(), "secret".to_string())]
        );
        assert!(headers.is_empty());
    }

    #[test]
    fn an_api_key_travels_as_a_header() {
        let mut config = StreamConfig::with_defaults();
        config.amazon_api_key = Some("key".into());
        let (query, headers) = resolver(config).amazon_credential().expect("credential");
        assert!(query.is_empty());
        assert_eq!(headers, vec![("X-API-Key".to_string(), "key".to_string())]);
    }

    #[test]
    fn a_bypass_token_outranks_an_api_key() {
        let mut config = StreamConfig::with_defaults();
        config.amazon_bypass_token = Some("secret".into());
        config.amazon_api_key = Some("key".into());
        let (query, _) = resolver(config).amazon_credential().expect("credential");
        assert_eq!(query[0].0, "bypass_token");
    }

    #[test]
    fn an_empty_credential_counts_as_absent() {
        let mut config = StreamConfig::with_defaults();
        config.amazon_bypass_token = Some(String::new());
        config.amazon_api_key = Some(String::new());
        let resolver = resolver(config);
        assert!(resolver.amazon_credential().is_none());
        assert!(!resolver.has_amazon_credential());
    }

    #[test]
    fn a_cached_jwt_is_used_when_no_static_credential_exists() {
        let resolver = resolver(StreamConfig::with_defaults());
        assert!(resolver.amazon_credential().is_none());
        resolver.cache_jwt("jwt-value".into());
        let (query, headers) = resolver.amazon_credential().expect("credential");
        assert!(query.is_empty());
        assert_eq!(headers[0].0, "X-Turnstile-JWT");
        assert!(resolver.has_amazon_credential());
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
        assert!(resolver.amazon_credential().is_none());
    }

    #[test]
    fn the_amazon_payload_is_found_at_any_nesting_level() {
        let flat = extract_track_payload(r#"{"asin":"B0DXYZ1234"}"#).expect("flat");
        assert_eq!(flat.asin.as_deref(), Some("B0DXYZ1234"));
        let nested = extract_track_payload(r#"{"data":{"asin":"B0DXYZ1234"}}"#).expect("nested");
        assert_eq!(nested.asin.as_deref(), Some("B0DXYZ1234"));
        let wrapped = extract_track_payload(r#"{"track":{"id":"B0DXYZ1234"}}"#).expect("wrapped");
        assert_eq!(wrapped.id.as_deref(), Some("B0DXYZ1234"));
    }

    #[test]
    fn an_unrecognisable_amazon_body_is_an_error() {
        assert!(extract_track_payload(r#"{"detail":"nope"}"#).is_err());
    }

    #[test]
    fn quality_tokens_match_the_web_client() {
        assert_eq!(Quality::HiRes.as_amazon(), "UHD");
        assert_eq!(Quality::Lossless.as_amazon(), "HD");
        assert_eq!(Quality::High.as_amazon(), "SD_HIGH");
        assert_eq!(Quality::Low.as_amazon(), "SD_LOW");
    }

    #[tokio::test]
    async fn a_track_without_an_isrc_never_reaches_deezer() {
        let mut config = StreamConfig::with_defaults();
        config.amazon_enabled = false;
        config.deezer_enabled = true;
        let resolver = resolver(config);
        let mut bare = track();
        bare.isrc = None;
        let error = resolver.resolve(&bare, Quality::Lossless).await;
        assert!(error.is_err());
    }

    #[tokio::test]
    async fn amazon_without_a_credential_reports_that_verification_is_needed() {
        let mut config = StreamConfig::with_defaults();
        config.deezer_enabled = false;
        let resolver = resolver(config);
        let error = resolver.resolve(&track(), Quality::Lossless).await;
        assert!(matches!(error, Err(ApiError::TurnstileRequired)));
    }
}
