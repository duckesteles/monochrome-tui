use monochrome_api::auth::AuthClient;
use monochrome_api::catalog::{ApiVersion, Catalog, Instance};
use monochrome_api::error::ApiError;
use monochrome_api::stream::{StreamConfig, StreamResolver};
use monochrome_core::library::SyncField;
use monochrome_core::model::{ArtistRef, Quality, Track};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn instance(server: &MockServer, version: ApiVersion) -> Instance {
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

    let catalog = Catalog::new(vec![
        instance(&broken, ApiVersion::new(2, 10)),
        instance(&healthy, ApiVersion::new(2, 10)),
    ])
    .expect("catalog");
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

    let catalog = Catalog::new(vec![
        instance(&broken, ApiVersion::new(2, 10)),
        instance(&healthy, ApiVersion::new(2, 10)),
    ])
    .expect("catalog");
    catalog.search_tracks("first").await.expect("first");
    catalog.search_tracks("second").await.expect("second");

    assert_eq!(
        catalog.active_instance().map(|i| i.url.clone()),
        Some(healthy.uri().trim_end_matches('/').to_string())
    );
}

fn playlist_page(offset: u32, count: usize, total: u32) -> serde_json::Value {
    let items: Vec<serde_json::Value> = (0..count)
        .map(|index| {
            json!({
                "item": {
                    "id": offset + index as u32,
                    "title": format!("Track {}", offset as usize + index),
                    "duration": 200
                },
                "type": "track"
            })
        })
        .collect();
    json!({
        "version": "2.10",
        "playlist": {
            "uuid": "abc-123",
            "title": "A Long Playlist",
            "numberOfTracks": total,
            "creator": { "name": "TIDAL" }
        },
        "items": items
    })
}

#[tokio::test]
async fn a_playlist_arrives_whole_rather_than_one_page_deep() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/playlist/"))
        .and(query_param("offset", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(playlist_page(100, 50, 150)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/playlist/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(playlist_page(0, 100, 150)))
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
    let (playlist, tracks) = catalog.playlist("abc-123").await.expect("playlist");
    assert_eq!(playlist.title, "A Long Playlist");
    assert_eq!(playlist.number_of_tracks, Some(150));
    assert_eq!(tracks.len(), 150, "the second page was never asked for");
    assert_eq!(tracks[0].id, 0);
    assert_eq!(tracks[149].id, 149);
}

#[tokio::test]
async fn a_service_that_ignores_the_offset_does_not_loop_forever() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/playlist/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(playlist_page(0, 100, 900)))
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
    let (_, tracks) = catalog.playlist("abc-123").await.expect("playlist");
    assert_eq!(
        tracks.len(),
        100,
        "a page that repeats itself must stop the walk, not extend it"
    );
}

#[tokio::test]
async fn a_playlist_wrapped_in_data_is_read_the_same_way() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/playlist/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": playlist_page(0, 4, 4) })),
        )
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
    let (playlist, tracks) = catalog.playlist("abc-123").await.expect("playlist");
    assert_eq!(playlist.uuid, "abc-123");
    assert_eq!(tracks.len(), 4);
}

#[tokio::test]
async fn an_album_longer_than_a_page_arrives_whole() {
    let server = MockServer::start().await;
    let page = |offset: u32, count: usize| {
        let items: Vec<serde_json::Value> = (0..count)
            .map(|index| {
                json!({ "item": { "id": offset + index as u32, "title": "T", "duration": 100 } })
            })
            .collect();
        json!({
            "version": "2.10",
            "data": { "id": 5, "title": "Everything", "numberOfTracks": 600, "items": items }
        })
    };
    Mock::given(method("GET"))
        .and(path("/album/"))
        .and(query_param("offset", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(500, 100)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/album/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 500)))
        .mount(&server)
        .await;

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
    let album = catalog.album(5).await.expect("album");
    assert_eq!(album.tracks.len(), 600);
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

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
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

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
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

    let catalog = Catalog::new(vec![
        instance(&first, ApiVersion::new(2, 10)),
        instance(&second, ApiVersion::new(2, 10)),
    ])
    .expect("catalog");
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

    let catalog = Catalog::new(vec![
        instance(&stranger, ApiVersion::new(2, 10)),
        instance(&serving, ApiVersion::new(2, 10)),
    ])
    .expect("catalog");
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

    let catalog = Catalog::new(vec![
        instance(&old, ApiVersion::new(2, 2)),
        instance(&new, ApiVersion::new(2, 6)),
    ])
    .expect("catalog");
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
    config.deezer_enabled = false;
    StreamResolver::new(config).expect("resolver")
}

fn envelope(resources: serde_json::Value) -> serde_json::Value {
    json!({
        "schema_version": "2.0",
        "request_id": "r1",
        "intent": "stream",
        "quality_requested": "LOSSLESS",
        "selected_source": "mono",
        "track": { "id": "t1", "duration_ms": 320_000 },
        "playback": resources,
    })
}

#[tokio::test]
async fn a_playback_lookup_returns_the_cdn_address_and_its_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .and(query_param("track", "One More Time"))
        .and(query_param("artist", "Daft Punk"))
        .and(query_param("isrc", "GBDUW0000053"))
        .and(query_param("duration", "320"))
        .and(query_param("intent", "stream"))
        .and(query_param("quality", "LOSSLESS"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer a-session-token",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([{
            "kind": "audio",
            "delivery": "direct",
            "source": "amazon",
            "quality": "HI_RES_LOSSLESS",
            "url": "https://cdn.example/audio.mp4",
            "encryption": { "key": { "value": "00112233445566778899aabbccddeeff" } }
        }]))))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_url = server.uri();
    config.playback_token = Some("a-session-token".into());
    config.deezer_enabled = false;
    let resolver = StreamResolver::new(config).expect("resolver");

    let handle = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect("stream");

    assert_eq!(handle.url, "https://cdn.example/audio.mp4");
    assert_eq!(
        handle.decryption_key.as_deref(),
        Some("00112233445566778899aabbccddeeff")
    );
    assert_eq!(handle.quality.as_deref(), Some("HI_RES_LOSSLESS"));
    assert_eq!(handle.source.label(), "amazon");
    assert!(
        handle.headers.is_empty(),
        "the cdn needs no gateway credential"
    );
}

#[tokio::test]
async fn an_isrc_is_sent_in_the_upper_case_the_service_expects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .and(query_param("isrc", "GBDUW0000053"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([{
            "kind": "audio",
            "delivery": "direct",
            "url": "https://cdn.example/track.flac"
        }]))))
        .expect(1)
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    let mut track = sample_track();
    track.isrc = Some("gbduw0000053".into());
    resolver
        .resolve(&track, Quality::Lossless)
        .await
        .expect("resolved");
}

#[tokio::test]
async fn a_manifest_this_player_cannot_read_is_passed_over() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([
            {
                "kind": "manifest",
                "delivery": "dash",
                "source": "amazon",
                "url": "https://cdn.example/stream.mpd"
            },
            {
                "kind": "audio",
                "delivery": "direct",
                "source": "mono",
                "url": "https://cdn.example/track.flac"
            }
        ]))))
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
}

#[tokio::test]
async fn an_envelope_with_nothing_this_player_can_read_defers_to_the_next_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([{
            "kind": "manifest",
            "delivery": "hls",
            "url": "https://cdn.example/stream.m3u8"
        }]))))
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/stream/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_url = server.uri();
    config.deezer_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");
    resolver.cache_jwt("a-session".into());

    let handle = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect("deezer takes over");
    assert_eq!(handle.source.label(), "deezer");
}

#[tokio::test]
async fn a_plaintext_stream_address_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([{
            "kind": "audio",
            "delivery": "direct",
            "url": "http://cdn.example/audio.mp4"
        }]))))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    assert!(
        resolver
            .resolve(&sample_track(), Quality::Lossless)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_schema_this_build_does_not_know_is_reported_rather_than_guessed_at() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": "3.0",
            "playback": [{
                "kind": "audio",
                "delivery": "direct",
                "url": "https://cdn.example/track.flac"
            }]
        })))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("an unknown schema is not guessed at");
    assert!(error.to_string().contains("schema 3.0"), "{error}");
}

#[tokio::test]
async fn a_rejected_token_is_discarded_and_reported_as_such() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "detail": "Invalid Turnstile JWT."
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_url = server.uri();
    config.playback_token = Some("secret".into());
    config.deezer_enabled = false;
    let resolver = StreamResolver::new(config).expect("resolver");

    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("the credential should be refused");
    assert!(matches!(error, ApiError::CredentialRejected));
}

#[tokio::test]
async fn a_blocked_address_is_named_as_such_rather_than_blamed_on_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    let error = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect_err("blocked");
    assert!(
        error.to_string().contains("blocked this address"),
        "{error}"
    );
    assert!(
        resolver.has_session(),
        "a block is not the session's fault and must not discard it"
    );
}

#[tokio::test]
async fn a_428_means_no_credential_was_sent_and_does_not_discard_a_good_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(428).set_body_json(json!({
            "error": "turnstile_required"
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.playback_url = server.uri();
    config.playback_token = Some("secret".into());
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
        "a 428 must not throw away a working session"
    );
}

#[tokio::test]
async fn validating_a_good_token_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([]))))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
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
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("bad-jwt".into());

    let error = resolver
        .validate_credential()
        .await
        .expect_err("the token should be refused");
    assert!(matches!(error, ApiError::CredentialRejected));
    assert!(!resolver.has_playback_credential());
}

#[tokio::test]
async fn validating_without_any_credential_asks_for_verification() {
    let mut config = StreamConfig::with_defaults();
    config.playback_url = "https://playback.invalid".into();
    let resolver = StreamResolver::new(config).expect("resolver");
    let error = resolver
        .validate_credential()
        .await
        .expect_err("no credential");
    assert!(matches!(error, ApiError::TurnstileRequired));
}

#[tokio::test]
async fn deezer_takes_over_when_playback_has_no_credential() {
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
async fn a_stream_that_measured_its_own_loudness_carries_it_back() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([{
            "kind": "audio",
            "delivery": "direct",
            "source": "amazon",
            "url": "https://cdn.example/track.mp4",
            "replay_gain": {
                "track_gain_db": null,
                "track_peak": null,
                "program_loudness_lufs": -19.4
            }
        }]))))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    resolver.cache_jwt("a-session".into());
    let handle = resolver
        .resolve(&sample_track(), Quality::Lossless)
        .await
        .expect("resolved");

    let measured = handle.loudness.expect("the stream measured itself");
    assert!((measured.gain_db - 1.4).abs() < 0.001, "{measured:?}");

    let mut track = sample_track();
    track.replay_gain = Some(-4.59);
    track.peak = Some(0.551);
    assert_eq!(
        handle.levelling(&track),
        (Some(measured.gain_db), Some(0.551))
    );
}

#[tokio::test]
async fn a_healthy_deezer_reports_how_many_accounts_it_still_has() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "accounts": { "total": 46, "available": 12, "dead": 34 }
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.deezer_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");
    let report = resolver.deezer_health().await.expect("health");
    assert!(report.ok);
    assert_eq!(report.accounts.available, 12);
    assert_eq!(report.accounts.total, 46);
}

#[tokio::test]
async fn a_deezer_with_no_accounts_left_is_not_reported_as_healthy() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "accounts": { "total": 46, "available": 0, "dead": 46 }
        })))
        .mount(&server)
        .await;

    let mut config = StreamConfig::with_defaults();
    config.deezer_url = server.uri();
    let resolver = StreamResolver::new(config).expect("resolver");
    let report = resolver.deezer_health().await.expect("health");
    assert!(!report.ok);
    assert_eq!(report.accounts.available, 0);
}

#[tokio::test]
async fn a_deezer_that_is_switched_off_is_not_even_asked() {
    let mut config = StreamConfig::with_defaults();
    config.deezer_enabled = false;
    config.deezer_url = "https://deezer.invalid".into();
    let resolver = StreamResolver::new(config).expect("resolver");
    assert!(matches!(
        resolver.deezer_health().await,
        Err(ApiError::NoSourceEnabled)
    ));
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
        .and(path("/api/auth/turnstile"))
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
async fn an_exchange_that_hands_back_nothing_is_not_stored_as_a_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/turnstile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "  " })))
        .mount(&server)
        .await;

    let resolver = playback_resolver(&server);
    assert!(resolver.finish_verification("cf-token").await.is_err());
    assert!(!resolver.has_session());
}

#[tokio::test]
async fn the_playback_service_answers_with_a_direct_address() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
        .and(wiremock::matchers::header("x-turnstile-jwt", "a-session"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(json!([{
            "kind": "audio",
            "delivery": "direct",
            "source": "mono",
            "url": "https://cdn.example/track.flac"
        }]))))
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
    assert_eq!(handle.quality.as_deref(), Some("LOSSLESS"));
    assert!(
        handle.decryption_key.is_none(),
        "the playback service serves plain flac"
    );
}

#[tokio::test]
async fn a_playback_session_that_is_refused_asks_for_the_browser_check_again() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
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
    Mock::given(method("GET"))
        .and(path("/api/v2/track/"))
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

    let catalog = Catalog::new(vec![instance(&dead, ApiVersion::new(2, 10))]).expect("catalog");
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

    let catalog = Catalog::new(vec![instance(&server, ApiVersion::new(2, 10))]).expect("catalog");
    let results = catalog.search("test").await.expect("the tracks came back");
    assert_eq!(results.tracks.len(), 1);
    assert!(results.albums.is_empty());
    assert!(results.artists.is_empty());
    assert!(results.playlists.is_empty());
}
