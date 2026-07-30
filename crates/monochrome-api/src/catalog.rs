use crate::cache::Cache;
use crate::error::{ApiError, ApiResult};
use crate::wire::*;
use monochrome_core::model::{Album, Artist, Playlist, Quality, Track};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_INSTANCES: &[(&str, f32)] = &[
    ("https://eu-central.monochrome.tf", 2.10),
    ("https://us-west.monochrome.tf", 2.10),
    ("https://api.monochrome.tf", 2.10),
    ("https://hifi.geeked.wtf", 2.7),
    ("https://monochrome-api.samidy.com", 2.3),
];

const CACHE_ENTRIES: usize = 128;
const CACHE_BYTES: usize = 4 * 1024 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub url: String,
    pub version: f32,
}

impl Instance {
    pub fn new(url: impl Into<String>, version: f32) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
            version,
        }
    }

    pub fn is_secure(&self) -> bool {
        crate::is_transport_allowed(&self.url)
    }
}

#[derive(Debug, Default, Clone)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

pub struct Catalog {
    client: reqwest::Client,
    instances: Vec<Instance>,
    preferred: Mutex<usize>,
    cache: Mutex<Cache<Arc<str>>>,
}

impl Catalog {
    pub fn new(instances: Vec<Instance>) -> ApiResult<Self> {
        let instances: Vec<Instance> = instances.into_iter().filter(Instance::is_secure).collect();
        if instances.is_empty() {
            return Err(ApiError::NoInstances);
        }
        let client = crate::http_client(REQUEST_TIMEOUT)?;
        Ok(Self {
            client,
            instances,
            preferred: Mutex::new(0),
            cache: Mutex::new(Cache::new(CACHE_ENTRIES, CACHE_BYTES, CACHE_TTL)),
        })
    }

    pub fn with_defaults() -> ApiResult<Self> {
        Self::new(
            DEFAULT_INSTANCES
                .iter()
                .map(|(url, version)| Instance::new(*url, *version))
                .collect(),
        )
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    pub fn active_instance(&self) -> Option<&Instance> {
        let index = *self.preferred.lock().expect("preferred");
        self.instances.get(index)
    }

    fn ordered(&self, min_version: Option<f32>) -> Vec<(usize, &Instance)> {
        let start = *self.preferred.lock().expect("preferred");
        let count = self.instances.len();
        (0..count)
            .map(|offset| {
                let index = (start + offset) % count;
                (index, &self.instances[index])
            })
            .filter(|(_, instance)| min_version.is_none_or(|min| instance.version >= min))
            .collect()
    }

    async fn fetch(&self, path: &str, min_version: Option<f32>) -> ApiResult<Arc<str>> {
        if let Some(hit) = self.cache.lock().expect("cache").get(path) {
            return Ok(hit);
        }

        let candidates = self.ordered(min_version);
        if candidates.is_empty() {
            return Err(ApiError::NoInstances);
        }

        let mut failures = Vec::new();
        for (index, instance) in candidates {
            let url = format!("{}{}", instance.url, path);
            match self.client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    let body = match response.text().await {
                        Ok(body) => body,
                        Err(error) => {
                            failures.push(format!("{}: {error}", instance.url));
                            continue;
                        }
                    };
                    let weight = body.len();
                    let body: Arc<str> = Arc::from(body);
                    self.cache.lock().expect("cache").insert(
                        path.to_string(),
                        body.clone(),
                        weight,
                    );
                    *self.preferred.lock().expect("preferred") = index;
                    return Ok(body);
                }
                Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                    return Err(ApiError::NotFound);
                }
                Ok(response) => {
                    failures.push(format!("{}: HTTP {}", instance.url, response.status()));
                }
                Err(error) => {
                    failures.push(format!("{}: {error}", instance.url));
                }
            }
        }
        Err(ApiError::AllInstancesFailed(failures))
    }

    async fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        min_version: Option<f32>,
    ) -> ApiResult<T> {
        let body = self.fetch(path, min_version).await?;
        serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))
    }

    pub async fn search_tracks(&self, query: &str) -> ApiResult<Vec<Track>> {
        let path = format!("/search/?s={}", encode(query));
        let envelope: Envelope<Page<WireTrack>> = self.fetch_json(&path, None).await?;
        Ok(envelope
            .data
            .items
            .into_iter()
            .map(WireTrack::into_core)
            .collect())
    }

    pub async fn search_albums(&self, query: &str) -> ApiResult<Vec<Album>> {
        let path = format!("/search/?al={}", encode(query));
        let envelope: Envelope<SearchSections> = self.fetch_json(&path, None).await?;
        Ok(envelope
            .data
            .albums
            .items
            .into_iter()
            .map(WireAlbum::into_core)
            .collect())
    }

    pub async fn search_artists(&self, query: &str) -> ApiResult<Vec<Artist>> {
        let path = format!("/search/?a={}", encode(query));
        let envelope: Envelope<SearchSections> = self.fetch_json(&path, None).await?;
        Ok(envelope
            .data
            .artists
            .items
            .into_iter()
            .map(WireArtist::into_core)
            .collect())
    }

    pub async fn search_playlists(&self, query: &str) -> ApiResult<Vec<Playlist>> {
        let path = format!("/search/?p={}", encode(query));
        let envelope: Envelope<SearchSections> = self.fetch_json(&path, None).await?;
        Ok(envelope
            .data
            .playlists
            .items
            .into_iter()
            .filter_map(WirePlaylist::into_core)
            .collect())
    }

    pub async fn search(&self, query: &str) -> SearchResults {
        let (tracks, albums, artists, playlists) = tokio::join!(
            self.search_tracks(query),
            self.search_albums(query),
            self.search_artists(query),
            self.search_playlists(query),
        );
        SearchResults {
            tracks: tracks.unwrap_or_default(),
            albums: albums.unwrap_or_default(),
            artists: artists.unwrap_or_default(),
            playlists: playlists.unwrap_or_default(),
        }
    }

    pub async fn track(&self, id: u64) -> ApiResult<Track> {
        let envelope: Envelope<WireTrack> =
            self.fetch_json(&format!("/info/?id={id}"), None).await?;
        Ok(envelope.data.into_core())
    }

    pub async fn tracks(&self, ids: &[u64]) -> Vec<Track> {
        let mut found = Vec::with_capacity(ids.len());
        for batch in ids.chunks(8) {
            let requests = batch.iter().map(|id| self.track(*id));
            found.extend(
                futures_util::future::join_all(requests)
                    .await
                    .into_iter()
                    .flatten(),
            );
        }
        found
    }

    pub async fn album(&self, id: u64) -> ApiResult<Album> {
        let envelope: Envelope<WireAlbum> = self
            .fetch_json(&format!("/album/?id={id}&limit=500"), None)
            .await?;
        Ok(envelope.data.into_core())
    }

    pub async fn artist(&self, id: u64) -> ApiResult<Artist> {
        let envelope: ArtistEnvelope = self.fetch_json(&format!("/artist/?id={id}"), None).await?;
        Ok(envelope.artist.into_core())
    }

    pub async fn artist_albums(&self, id: u64) -> ApiResult<Vec<Album>> {
        let envelope: ArtistAlbumsEnvelope = self
            .fetch_json(&format!("/artist/?f={id}&skip_tracks=true"), None)
            .await?;
        Ok(envelope
            .albums
            .items
            .into_iter()
            .map(WireAlbum::into_core)
            .collect())
    }

    pub async fn artist_top_tracks(&self, id: u64) -> ApiResult<Vec<Track>> {
        let envelope: ArtistAlbumsEnvelope =
            self.fetch_json(&format!("/artist/?f={id}"), None).await?;
        Ok(envelope
            .tracks
            .into_iter()
            .map(WireTrack::into_core)
            .collect())
    }

    pub async fn playlist(&self, uuid: &str) -> ApiResult<(Playlist, Vec<Track>)> {
        let envelope: Envelope<PlaylistEnvelope> = self
            .fetch_json(&format!("/playlist/?id={}", encode(uuid)), None)
            .await?;
        let tracks = envelope
            .data
            .items
            .into_iter()
            .map(|entry| entry.item.into_core())
            .collect();
        let playlist = envelope
            .data
            .playlist
            .into_core()
            .ok_or_else(|| ApiError::Decode("playlist is missing its uuid".into()))?;
        Ok((playlist, tracks))
    }

    pub async fn recommendations(&self, track_id: u64) -> ApiResult<Vec<Track>> {
        let envelope: Envelope<Page<RecommendationItem>> = self
            .fetch_json(&format!("/recommendations/?id={track_id}"), Some(2.4))
            .await?;
        Ok(envelope
            .data
            .items
            .into_iter()
            .filter_map(|entry| entry.into_track().map(WireTrack::into_core))
            .collect())
    }

    pub async fn track_manifest(&self, id: u64, quality: Quality) -> ApiResult<ManifestAttributes> {
        let path = format!(
            "/trackManifests/?id={id}&quality={}&adaptive=false&formats=flac",
            quality.as_tidal()
        );
        let envelope: Envelope<ManifestEnvelope> = self.fetch_json(&path, None).await?;
        Ok(envelope.data.data.attributes)
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct RecommendationItem {
    #[serde(default)]
    track: Option<WireTrack>,
    #[serde(flatten)]
    inline: Option<WireTrack>,
}

impl RecommendationItem {
    fn into_track(self) -> Option<WireTrack> {
        self.track.or(self.inline)
    }
}

fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_escapes_reserved_characters() {
        assert_eq!(encode("daft punk"), "daft+punk");
        assert_eq!(encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode("Beyoncé"), "Beyonc%C3%A9");
        assert_eq!(encode("m/s?x#y"), "m%2Fs%3Fx%23y");
    }

    #[test]
    fn instance_urls_lose_their_trailing_slash() {
        assert_eq!(
            Instance::new("https://a.example/", 1.0).url,
            "https://a.example"
        );
    }

    #[test]
    fn plaintext_instances_are_rejected() {
        let error = Catalog::new(vec![Instance::new("http://insecure.example", 2.0)]);
        assert!(matches!(error, Err(ApiError::NoInstances)));
    }

    #[test]
    fn plaintext_loopback_instances_are_allowed_for_self_hosting() {
        assert!(Instance::new("http://127.0.0.1:8080", 2.0).is_secure());
        assert!(Instance::new("http://localhost:8080", 2.0).is_secure());
        assert!(!Instance::new("http://192.168.1.5:8080", 2.0).is_secure());
    }

    #[test]
    fn defaults_are_all_secure() {
        let catalog = Catalog::with_defaults().expect("catalog");
        assert!(catalog.instances().iter().all(Instance::is_secure));
        assert_eq!(catalog.instances().len(), DEFAULT_INSTANCES.len());
    }

    #[test]
    fn version_filtering_excludes_older_instances() {
        let catalog = Catalog::new(vec![
            Instance::new("https://old.example", 2.2),
            Instance::new("https://new.example", 2.6),
        ])
        .expect("catalog");
        let eligible = catalog.ordered(Some(2.4));
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].1.url, "https://new.example");
    }

    #[test]
    fn a_recommendation_the_server_did_not_fill_in_is_skipped_not_fatal() {
        let wrapped: RecommendationItem =
            serde_json::from_str(r#"{"track":{"id":7,"title":"Rec","duration":90}}"#)
                .expect("parses");
        assert_eq!(wrapped.into_track().map(|track| track.id), Some(7));

        let inline: RecommendationItem =
            serde_json::from_str(r#"{"id":9,"title":"Rec","duration":90}"#).expect("parses");
        assert_eq!(inline.into_track().map(|track| track.id), Some(9));

        let empty: RecommendationItem = serde_json::from_str(r#"{"note":"nothing here"}"#)
            .expect("an unexpected shape must still parse");
        assert!(
            empty.into_track().is_none(),
            "a malformed entry must be skipped rather than crash the client"
        );
    }

    #[test]
    fn failover_order_starts_from_the_preferred_instance() {
        let catalog = Catalog::new(vec![
            Instance::new("https://a.example", 2.0),
            Instance::new("https://b.example", 2.0),
            Instance::new("https://c.example", 2.0),
        ])
        .expect("catalog");
        *catalog.preferred.lock().unwrap() = 1;
        let order: Vec<&str> = catalog
            .ordered(None)
            .iter()
            .map(|(_, i)| i.url.as_str())
            .collect();
        assert_eq!(
            order,
            vec![
                "https://b.example",
                "https://c.example",
                "https://a.example"
            ]
        );
    }
}
