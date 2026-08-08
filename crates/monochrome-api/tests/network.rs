use monochrome_api::auth::AuthClient;
use monochrome_api::catalog::{Catalog, Instance};
use monochrome_api::error::ApiError;
use monochrome_api::stream::{StreamConfig, StreamResolver};
use monochrome_core::library::SyncField;
use monochrome_core::model::{ArtistRef, Quality, Track};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn instance(server: &MockServer, version: f32) -> Instance {
    Instance::new(server.uri(), version)
}

fn track_page() -> serde_json::Value {
    json!({
        "version": "2.10",
        "data": {
            "limit": 25,
            "offset": 0,
            "totalNumberOfItems": 1,
            "items": [{
                "id": 42,
                "title": "Test Track",
                "duration": 180,
                "isrc": "AAAAA0000001",
                "artist": { "id": 1, "name": "Tester" }
            }]
        }
    })
}

#[tokio::test]
async fn a_failing_instance_is_skipped_for_a_healthy_one() {
    let broken = MockServer::start().await;
    let healthy = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&broken)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(track_page()))
        .mount(&healthy)
        .await;

    let catalog =
        Catalog::new(vec![instance(&broken, 2.10), instance(&healthy, 2.10)]).expect("catalog");
    let tracks = catalog.search_tracks("test").await.expect("tracks");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title, "Test Track");
}

#[tokio::test]
async fn the_healthy_instance_becomes_preferred_after_a_failover() {
    let broken = MockServer::start().await;
    let healthy = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&broken)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(track_page()))
        .mount(&healthy)
        .await;

    let catalog =
        Catalog::new(vec![instance(&broken, 2.10), instance(&healthy, 2.10)]).expect("catalog");
    catalog.search_tracks("first").await.expect("first");
    catalog.search_tracks("second").await.expect("second");

    assert_eq!(
        catalog.active_instance().map(|i| i.url.clone()),
        Some(healthy.uri().trim_end_matches('/').to_string())
    );
}

#[tokio::test]
async fn a_repeated_request_is_served_from_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(track_page()))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, 2.10)]).expect("catalog");
    catalog.search_tracks("same").await.expect("first");
    catalog.search_tracks("same").await.expect("second");
}

#[tokio::test]
async fn every_instance_failing_is_reported_as_such() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, 2.10)]).expect("catalog");
    let error = catalog.search_tracks("x").await.expect_err("should fail");
    assert!(matches!(error, ApiError::AllInstancesFailed(_)));
    assert!(error.to_string().contains("every catalog instance failed"));
}

#[tokio::test]
async fn a_resource_no_instance_has_is_reported_as_missing() {
    let first = MockServer::start().await;
    let second = MockServer::start().await;
    for server in [&first, &second] {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(server)
            .await;
    }

    let catalog =
        Catalog::new(vec![instance(&first, 2.10), instance(&second, 2.10)]).expect("catalog");
    let error = catalog.track(1).await.expect_err("should fail");
    assert!(matches!(error, ApiError::NotFound), "{error}");
}

#[tokio::test]
async fn an_instance_that_does_not_serve_a_route_falls_through_to_one_that_does() {
    let stranger = MockServer::start().await;
    let serving = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&stranger)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(track_page()))
        .mount(&serving)
        .await;

    let catalog =
        Catalog::new(vec![instance(&stranger, 2.10), instance(&serving, 2.10)]).expect("catalog");
    let tracks = catalog.search_tracks("test").await.expect("tracks");
    assert_eq!(tracks.len(), 1);
}

#[tokio::test]
async fn recommendations_skip_instances_below_the_required_version() {
    let old = MockServer::start().await;
    let new = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(track_page()))
        .expect(0)
        .mount(&old)
        .await;
    Mock::given(method("GET"))
        .and(path("/recommendations/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": "2.10",
            "data": { "items": [{ "track": { "id": 7, "title": "Rec", "duration": 100 } }] }
        })))
        .mount(&new)
        .await;

    let catalog = Catalog::new(vec![instance(&old, 2.2), instance(&new, 2.6)]).expect("catalog");
    let tracks = catalog.recommendations(1).await.expect("recommendations");
    assert_eq!(tracks[0].id, 7);
}

#[tokio::test]
async fn signing_in_returns_the_token_and_the_user() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/sign-in/email"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "session-token",
            "user": { "id": "u1", "email": "a@b.co", "name": "Ada" }
        })))
        .mount(&server)
        .await;

    let client = AuthClient::new(server.uri()).expect("client");
    let (token, user) = client.sign_in("a@b.co", "pw").await.expect("sign in");
    assert_eq!(token, "session-token");
    assert_eq!(user.display_name(), "Ada");
}

#[tokio::test]
async fn wrong_credentials_surface_the_server_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "message": "Invalid email or password",
            "code": "INVALID_EMAIL_OR_PASSWORD"
        })))
        .mount(&server)
        .await;

    let client = AuthClient::new(server.uri()).expect("client");
    let error = client
        .sign_in("a@b.co", "wrong")
        .await
        .expect_err("rejected");
    assert!(error.to_string().contains("Invalid email or password"));
}

#[tokio::test]
async fn a_dead_session_is_reported_as_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = AuthClient::new(server.uri()).expect("client");
    let error = client.me("stale").await.expect_err("unauthorized");
    assert!(matches!(error, ApiError::Unauthorized));
}

#[tokio::test]
async fn the_sync_document_is_loaded_with_a_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/sync"))
        .and(header("authorization", "Bearer session-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "appUserId": "u1",
            "library": { "tracks": { "42": { "id": 42, "title": "Saved" } } },
            "history": [],
            "userPlaylists": {},
            "userFolders": {}
        })))
        .mount(&server)
        .await;

    let client = AuthClient::new(server.uri()).expect("client");
    let document = client.load_sync("session-token").await.expect("sync");
    assert_eq!(document.app_user_id.as_deref(), Some("u1"));
    assert_eq!(document.library["tracks"]["42"]["title"], json!("Saved"));
}

#[tokio::test]
async fn only_changed_fields_are_pushed() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/sync"))
        .and(wiremock::matchers::body_json(json!({
            "history": [{ "id": 1, "timestamp": 5 }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "history": [] })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AuthClient::new(server.uri()).expect("client");
    client
        .push_sync(
            "token",
            &[(SyncField::History, json!([{ "id": 1, "timestamp": 5 }]))],
        )
        .await
        .expect("push");
}

fn sample_track() -> Track {
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

fn playback_resolver(server: &MockServer) -> StreamResolver {
    let mut config = StreamConfig::with_defaults();
    config.playback_url = server.uri();
    config.amazon_enabled = false;
    config.deezer_enabled = false;
    StreamResolver::new(config).expect("resolver")
}

fn resolver_for(server: &MockServer, bypass: Option<&str>) -> StreamResolver {
    let mut config = StreamConfig::with_defaults();
    config.playback_enabled = false;
    config.amazon_url = server.uri();
    config.deezer_url = server.uri();
    config.amazon_bypass_token = bypass.map(str::to_string);
    StreamResolver::new(config).expect("resolver")
}

#[tokio::test]
async fn an_amazon_lookup_returns_the_cdn_address_and_its_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .and(query_param("bypass_token", "secret"))
        .and(query_param("quality", "HD"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asin": "B0DXYZ1234",
            "quality_selected": "HD_44",
            "stream_url": "https://cdn.example/audio.mp4",
            "decryption_key": "00112233445566778899aabbccddeeff"
        })))
        .mount(&server)
        .await;

    let resolver = resolver_for(&server, Some("secret"));
    let handle = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect("stream");

    assert_eq!(handle.url, "https://cdn.example/audio.mp4");
    assert_eq!(
        handle.decryption_key.as_deref(),
        Some("00112233445566778899aabbccddeeff")
    );
    assert_eq!(handle.quality.as_deref(), Some("HD_44"));
    assert_eq!(handle.source.label(), "amazon");
    assert!(
        handle.headers.is_empty(),
        "the cdn needs no gateway credential"
    );
}

#[tokio::test]
async fn a_lookup_without_a_stream_address_is_an_error_not_a_silent_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asin": "B0DXYZ1234",
            "quality_selected": "HD"
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_enabled = false;
    config.amazon_url = server.uri();
    config.amazon_bypass_token = Some("secret".into());
    config.deezer_enabled = false;
    let resolver = StreamResolver::new(config).expect("resolver");

    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("no address means no playback");
    assert!(error.to_string().contains("no stream address"));
}

#[tokio::test]
async fn a_plaintext_stream_address_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asin": "B0DXYZ1234",
            "stream_url": "http://cdn.example/audio.mp4"
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.amazon_url = server.uri();
    config.amazon_bypass_token = Some("secret".into());
    config.deezer_enabled = false;
    let resolver = StreamResolver::new(config).expect("resolver");

    assert!(
        resolver
            .resolve(&sample_track(), Quality::Lossless)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_rejected_token_is_discarded_and_reported_as_such() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "detail": "Invalid Turnstile JWT."
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_enabled = false;
    config.amazon_url = server.uri();
    config.amazon_bypass_token = Some("secret".into());
    config.deezer_enabled = false;
    let resolver = StreamResolver::new(config).expect("resolver");

    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("the credential should be refused");
    assert!(matches!(error, ApiError::CredentialRejected));
}

#[tokio::test]
async fn a_428_means_no_credential_was_sent_and_does_not_discard_a_good_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .respond_with(ResponseTemplate::new(428).set_body_json(json!({
            "error": "turnstile_required"
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_enabled = false;
    config.amazon_url = server.uri();
    config.amazon_bypass_token = Some("secret".into());
    config.deezer_enabled = false;
    let resolver = StreamResolver::new(config).expect("resolver");
    resolver.cache_jwt("good-session".into());

    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("verification is needed");
    assert!(matches!(error, ApiError::TurnstileRequired));
    assert!(
        resolver.has_session(),
        "a 428 from amazon must not throw away a working session"
    );
}

#[tokio::test]
async fn validating_a_good_token_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "asin": "B0DXYZ1234" })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.amazon_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");
    resolver.cache_jwt("fresh-jwt".into());
    resolver
        .validate_credential()
        .await
        .expect("token accepted");
}

#[tokio::test]
async fn validating_a_bad_token_reports_it_before_playback_is_attempted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/track/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.amazon_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");
    resolver.cache_jwt("bad-jwt".into());

    let error = resolver
        .validate_credential()
        .await
        .expect_err("the token should be refused");
    assert!(matches!(error, ApiError::CredentialRejected));
    assert!(!resolver.has_amazon_credential());
}

#[tokio::test]
async fn validating_without_any_credential_asks_for_verification() {
    let mut config = StreamConfig::with_defaults();
    config.amazon_url = "https://amazon.invalid".into();
    let resolver = StreamResolver::new(config).expect("resolver");
    let error = resolver
        .validate_credential()
        .await
        .expect_err("no credential");
    assert!(matches!(error, ApiError::TurnstileRequired));
}

#[tokio::test]
async fn deezer_takes_over_when_amazon_has_no_credential() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/stream/"))
        .and(query_param("isrc", "GBDUW0000053"))
        .and(query_param("format", "FLAC"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_enabled = false;
    config.amazon_enabled = false;
    config.deezer_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");

    let handle = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect("stream");
    assert_eq!(handle.source.label(), "deezer");
    assert!(handle.url.contains("format=FLAC"));
    assert!(
        handle.decryption_key.is_none(),
        "deezer streams are not encrypted"
    );
}

#[tokio::test]
async fn a_dead_deezer_gateway_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_enabled = false;
    config.amazon_enabled = false;
    config.deezer_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");

    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("dead gateway");
    assert!(matches!(error, ApiError::Status { code: 503, .. }));
}

#[tokio::test]
async fn exchanging_a_challenge_token_caches_the_jwt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/turnstile"))
        .and(wiremock::matchers::body_json(
            json!({ "turnstile_token": "cf-token" }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "fresh-session", "expires_in": 3600 })),
        )
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    assert!(!resolver.has_session());
    resolver
        .finish_verification("cf-token")
        .await
        .expect("exchange");
    assert!(resolver.has_session());
    assert_eq!(resolver.cached_jwt().as_deref(), Some("fresh-session"));
}

#[tokio::test]
async fn the_playback_service_answers_with_a_direct_address() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/playback"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer a-session",
        ))
        .and(wiremock::matchers::body_json(json!({
            "song_name": "One More Time",
            "artist": "Daft Punk",
            "isrc": "GBDUW0000053",
            "duration": 320
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "url": "https://cdn.example/track.flac",
            "track_id": "t1",
            "recording_id": "r1",
            "title": "One More Time",
            "artists": ["Daft Punk"]
        })))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    let handle = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect("resolved");

    assert_eq!(handle.url, "https://cdn.example/track.flac");
    assert_eq!(handle.source.label(), "monochrome");
    assert!(
        handle.decryption_key.is_none(),
        "the playback service serves plain flac"
    );
}

#[tokio::test]
async fn a_playback_session_that_is_refused_asks_for_the_browser_check_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/playback"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("stale".into());
    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("refused");

    assert!(matches!(error, ApiError::TurnstileRequired));
    assert!(
        !resolver.has_session(),
        "a refused session must not be kept"
    );
}

#[tokio::test]
async fn without_a_session_the_playback_service_is_not_even_asked() {
    let server = MockServer::start().await;
    let resolver = playback_resolver(&server);
    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("no session");
    assert!(matches!(error, ApiError::TurnstileRequired));
}

#[tokio::test]
async fn being_rate_limited_is_reported_in_plain_words() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/playback"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("rate limited");
    assert!(error.to_string().contains("rate limiting"), "{error}");
}

#[tokio::test]
async fn a_search_that_reached_nothing_is_reported_rather_than_looking_empty() {
    let dead = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&dead)
        .await;

    let catalog = Catalog::new(vec![instance(&dead, 2.10)]).expect("catalog");
    let error = catalog
        .search("test")
        .await
        .expect_err("an unreachable catalog must not read as zero matches");
    assert!(matches!(error, ApiError::AllInstancesFailed(_)), "{error}");
}

#[tokio::test]
async fn a_search_section_that_fails_alone_does_not_sink_the_whole_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("s", "test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(track_page()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, 2.10)]).expect("catalog");
    let results = catalog.search("test").await.expect("the tracks came back");
    assert_eq!(results.tracks.len(), 1);
    assert!(results.albums.is_empty());
    assert!(results.artists.is_empty());
    assert!(results.playlists.is_empty());
}
