use monochrome_api::wire::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/{name}.json")).expect("fixture is readable")
}

fn parse<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&fixture(name)).expect("fixture parses")
}

#[test]
fn a_live_track_search_response_parses() {
    let envelope: Envelope<Page<WireTrack>> = parse("search_tracks");
    let tracks: Vec<_> = envelope
        .data
        .items
        .into_iter()
        .map(WireTrack::into_core)
        .collect();
    assert!(!tracks.is_empty());
    let first = &tracks[0];
    assert_eq!(first.id, 1550546);
    assert_eq!(first.title, "One More Time");
    assert_eq!(first.duration, 320);
    assert_eq!(first.artist_name(), "Daft Punk");
    assert_eq!(first.album_title(), "Discovery");
    assert_eq!(first.isrc.as_deref(), Some("GBDUW0000053"));
    assert!(first.stream_ready);
}

#[test]
fn a_live_album_search_response_parses() {
    let envelope: Envelope<SearchSections> = parse("search_albums");
    let albums: Vec<_> = envelope
        .data
        .albums
        .items
        .into_iter()
        .map(WireAlbum::into_core)
        .collect();
    assert!(!albums.is_empty());
    let discovery = albums
        .iter()
        .find(|album| album.id == 1550545)
        .expect("Discovery is in the results");
    assert_eq!(discovery.title, "Discovery");
    assert_eq!(discovery.year(), Some("2001"));
    assert_eq!(discovery.number_of_tracks, Some(14));
    assert_eq!(discovery.artist_name(), "Daft Punk");
}

#[test]
fn a_live_artist_search_response_parses() {
    let envelope: Envelope<SearchSections> = parse("search_artists");
    let artists: Vec<_> = envelope
        .data
        .artists
        .items
        .into_iter()
        .map(WireArtist::into_core)
        .collect();
    assert_eq!(artists[0].id, 8847);
    assert_eq!(artists[0].name, "Daft Punk");
    assert!(artists[0].picture.is_some());
}

#[test]
fn a_live_playlist_search_response_parses() {
    let envelope: Envelope<SearchSections> = parse("search_playlists");
    let playlists: Vec<_> = envelope
        .data
        .playlists
        .items
        .into_iter()
        .filter_map(WirePlaylist::into_core)
        .collect();
    assert!(!playlists.is_empty());
    assert!(!playlists[0].uuid.is_empty());
    assert!(playlists[0].number_of_tracks.unwrap_or(0) > 0);
}

#[test]
fn a_live_track_info_response_parses() {
    let envelope: Envelope<WireTrack> = parse("track_info");
    let track = envelope.data.into_core();
    assert_eq!(track.id, 1550546);
    assert_eq!(track.replay_gain, Some(-6.83));
    assert_eq!(track.track_number, Some(1));
}

#[test]
fn a_live_album_response_carries_its_tracks() {
    let envelope: Envelope<WireAlbum> = parse("album");
    let album = envelope.data.into_core();
    assert_eq!(album.id, 1550545);
    assert_eq!(album.title, "Discovery");
    assert_eq!(album.tracks.len(), 14);
    assert_eq!(album.tracks[0].title, "One More Time");
    assert_eq!(album.tracks[13].track_number, Some(14));
}

#[test]
fn a_live_artist_response_parses() {
    let envelope: ArtistEnvelope = parse("artist");
    let artist = envelope.artist.into_core();
    assert_eq!(artist.id, 8847);
    assert_eq!(artist.name, "Daft Punk");
}

#[test]
fn a_live_artist_albums_response_parses() {
    let envelope: ArtistAlbumsEnvelope = parse("artist_albums");
    let albums: Vec<_> = envelope
        .albums
        .items
        .into_iter()
        .map(WireAlbum::into_core)
        .collect();
    assert!(!albums.is_empty());
    assert!(albums.iter().all(|album| !album.title.is_empty()));
}

#[test]
fn a_live_manifest_response_exposes_its_presentation() {
    let envelope: Envelope<ManifestEnvelope> = parse("track_manifest");
    let attributes = envelope.data.data.attributes;
    assert_eq!(attributes.presentation.as_deref(), Some("PREVIEW"));
    assert!(attributes.uri.is_some());
    assert_eq!(attributes.formats, vec!["FLAC".to_string()]);
}
