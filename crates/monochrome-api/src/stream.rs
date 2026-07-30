use crate::error::{ApiError, ApiResult};
use crate::turnstile::{self, Bridge};
use monochrome_core::model::{Quality, Track};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_AMAZON_URL: &str = "https://amz.geeked.wtf";
const WEB_ORIGIN: &str = "https://monochrome.tf";
pub const DEFAULT_DEEZER_URL: &str = "https://dzr.tabs-vs-spaces.wtf";
const JWT_LIFETIME: Duration = Duration::from_secs(55 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Amazon,
    Deezer,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
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
    pub amazon_enabled: bool,
    pub amazon_url: String,
    pub amazon_bypass_token: Option<String>,
    pub amazon_api_key: Option<String>,
    pub turnstile_site_key: String,
    pub deezer_enabled: bool,
    pub deezer_url: String,
}

impl StreamConfig {
    pub fn with_defaults() -> Self {
        Self {
            amazon_enabled: true,
            amazon_url: DEFAULT_AMAZON_URL.into(),
            amazon_bypass_token: None,
            amazon_api_key: None,
            turnstile_site_key: turnstile::DEFAULT_SITE_KEY.into(),
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
}

#[derive(Debug, Clone)]
struct CachedJwt {
    token: String,
    obtained: Instant,
}

impl CachedJwt {
    fn is_valid(&self) -> bool {
        self.obtained.elapsed() < JWT_LIFETIME
    }
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
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(crate::USER_AGENT)
            .build()?;
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
        *self.jwt.lock().expect("jwt") = Some(CachedJwt {
            token,
            obtained: Instant::now(),
        });
    }

    pub fn cached_jwt(&self) -> Option<String> {
        self.jwt
            .lock()
            .expect("jwt")
            .as_ref()
            .filter(|jwt| jwt.is_valid())
            .map(|jwt| jwt.token.clone())
    }

    pub fn has_amazon_credential(&self) -> bool {
        self.config
            .amazon_bypass_token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
            || self
                .config
                .amazon_api_key
                .as_ref()
                .is_some_and(|k| !k.is_empty())
            || self
                .jwt
                .lock()
                .expect("jwt")
                .as_ref()
                .is_some_and(CachedJwt::is_valid)
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
        Bridge::bind(&self.config.turnstile_site_key).await
    }

    pub async fn finish_verification(&self, challenge_token: &str) -> ApiResult<()> {
        let base = self.config.amazon_url.trim_end_matches('/');
        let response = self
            .client
            .post(format!("{base}/api/auth/turnstile"))
            .json(&serde_json::json!({ "cf_turnstile_response": challenge_token }))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(ApiError::Status {
                code: status.as_u16(),
                message: "verification was rejected".into(),
            });
        }
        let parsed: TurnstileExchange =
            serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))?;
        self.cache_jwt(parsed.access_token);
        Ok(())
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

        let status = request.send().await?.status().as_u16();
        match status {
            401 => {
                self.jwt.lock().expect("jwt").take();
                Err(ApiError::CredentialRejected)
            }
            428 => Err(ApiError::TurnstileRequired),
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
            return "stored turnstile token";
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
            return Err(ApiError::Status {
                code: status.as_u16(),
                message: "amazon lookup failed".into(),
            });
        }
        serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))
    }

    pub async fn resolve(&self, track: &Track, quality: Quality) -> ApiResult<StreamHandle> {
        let mut last = ApiError::Network("no source is enabled".into());

        if self.config.amazon_enabled {
            match self.resolve_amazon(track, quality).await {
                Ok(handle) => return Ok(handle),
                Err(error) => last = error,
            }
        }

        if self.config.deezer_enabled
            && let Some(isrc) = track.isrc.as_deref()
        {
            match self.resolve_deezer(isrc, quality).await {
                Ok(handle) => return Ok(handle),
                Err(error) => {
                    if !matches!(
                        last,
                        ApiError::TurnstileRequired | ApiError::CredentialRejected
                    ) {
                        last = error;
                    }
                }
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
            return Err(ApiError::Status {
                code: status.as_u16(),
                message: "amazon lookup failed".into(),
            });
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
        });
        assert_eq!(resolver.cached_jwt(), None);
    }

    #[test]
    fn an_expired_jwt_is_ignored() {
        let resolver = resolver(StreamConfig::with_defaults());
        *resolver.jwt.lock().unwrap() = Some(CachedJwt {
            token: "old".into(),
            obtained: Instant::now() - JWT_LIFETIME - Duration::from_secs(1),
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
