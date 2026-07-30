use crate::library::*;
use crate::model::*;
use serde_json::{Value, json};

fn track(id: u64) -> Track {
    Track {
        id,
        title: format!("Song {id}"),
        duration: 200,
        explicit: false,
        artist: Some(ArtistRef {
            id: 7,
            name: "Artist".into(),
            picture: None,
        }),
        artists: vec![ArtistRef {
            id: 7,
            name: "Artist".into(),
            picture: None,
        }],
        album: Some(AlbumRef {
            id: 5,
            title: "Album".into(),
            cover: Some("cover-id".into()),
            release_date: Some("2001-03-12".into()),
            artist: None,
            number_of_tracks: Some(14),
        }),
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

fn library_with(existing: Value) -> Library {
    Library::new(SyncDocument {
        library: existing,
        ..Default::default()
    })
}

#[test]
fn favoriting_a_track_writes_the_upstream_shape() {
    let mut library = Library::default();
    library.set_favorite_track(&track(1), true, 1_700_000_000_000);
    let entry = &library.document().library["tracks"]["1"];
    assert_eq!(entry["id"], json!(1));
    assert_eq!(entry["addedAt"], json!(1_700_000_000_000u64));
    assert_eq!(entry["title"], json!("Song 1"));
    assert_eq!(entry["album"]["cover"], json!("cover-id"));
    assert_eq!(entry["album"]["numberOfTracks"], json!(14));
    assert_eq!(entry["artists"][0]["name"], json!("Artist"));
    assert_eq!(entry["isrc"], json!("GBDUW0000053"));
}

#[test]
fn unfavoriting_removes_only_that_key() {
    let mut library = library_with(json!({
        "tracks": { "1": { "id": 1 }, "2": { "id": 2 } }
    }));
    library.set_favorite_track(&track(1), false, 0);
    assert!(!library.is_favorite(FavoriteKind::Track, "1"));
    assert!(library.is_favorite(FavoriteKind::Track, "2"));
}

#[test]
fn unknown_library_sections_survive_a_write() {
    let mut library = library_with(json!({
        "mixes": { "abc": { "id": "abc", "title": "Mix" } },
        "videos": { "9": { "id": 9 } }
    }));
    library.set_favorite_track(&track(1), true, 1);
    let stored = &library.document().library;
    assert_eq!(stored["mixes"]["abc"]["title"], json!("Mix"));
    assert_eq!(stored["videos"]["9"]["id"], json!(9));
}

#[test]
fn unknown_fields_inside_a_kept_entry_survive() {
    let mut library = library_with(json!({
        "tracks": { "2": { "id": 2, "somethingNew": true } }
    }));
    library.set_favorite_track(&track(1), true, 1);
    assert_eq!(
        library.document().library["tracks"]["2"]["somethingNew"],
        json!(true)
    );
}

#[test]
fn section_names_report_what_the_server_actually_stores() {
    let library = library_with(json!({
        "tracks": { "1": { "id": 1 }, "2": { "id": 2 } },
        "mixes": { "a": { "id": "a" } }
    }));
    let mut sections = library.section_names();
    sections.sort();
    assert_eq!(
        sections,
        vec!["mixes: 1".to_string(), "tracks: 2".to_string()]
    );
}

#[test]
fn section_names_are_empty_for_a_malformed_document() {
    assert!(library_with(json!("nonsense")).section_names().is_empty());
}

#[test]
fn favorites_are_listed_newest_first() {
    let mut library = Library::default();
    library.set_favorite_track(&track(1), true, 100);
    library.set_favorite_track(&track(2), true, 300);
    library.set_favorite_track(&track(3), true, 200);
    let ids: Vec<u64> = library.favorite_tracks().iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![2, 3, 1]);
}

#[test]
fn playlists_are_keyed_by_uuid() {
    let mut library = Library::default();
    let playlist = Playlist {
        uuid: "e031991d".into(),
        title: "Chill".into(),
        image: None,
        number_of_tracks: Some(111),
        duration: None,
        creator_name: Some("TIDAL".into()),
        description: None,
    };
    library.set_favorite_playlist(&playlist, true, 5);
    assert!(library.is_favorite(FavoriteKind::Playlist, "e031991d"));
    assert_eq!(
        library.document().library["playlists"]["e031991d"]["user"]["name"],
        json!("TIDAL")
    );
}

#[test]
fn recording_a_play_prepends_with_a_timestamp() {
    let mut library = Library::default();
    library.record_play(&track(1), 1_000);
    let entry = &library.document().history[0];
    assert_eq!(entry["id"], json!(1));
    assert_eq!(entry["timestamp"], json!(1_000));
}

#[test]
fn replaying_a_track_moves_it_to_the_front_without_duplicating() {
    let mut library = Library::default();
    library.record_play(&track(1), 1_000);
    library.record_play(&track(2), 2_000);
    library.record_play(&track(1), 3_000);
    let history = library.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, 1);
    assert_eq!(history[1].id, 2);
}

#[test]
fn history_timestamps_stay_strictly_increasing_under_a_frozen_clock() {
    let mut library = Library::default();
    library.record_play(&track(1), 500);
    library.record_play(&track(2), 500);
    let history = &library.document().history;
    let newest = history[0]["timestamp"].as_u64().unwrap();
    let older = history[1]["timestamp"].as_u64().unwrap();
    assert!(newest > older, "{newest} should exceed {older}");
}

#[test]
fn history_is_capped_at_one_hundred_entries() {
    let mut library = Library::default();
    for id in 1..=120 {
        library.record_play(&track(id), 1_000 + id);
    }
    assert_eq!(library.history().len(), HISTORY_LIMIT);
    assert_eq!(library.history()[0].id, 120);
}

#[test]
fn clearing_history_empties_the_list() {
    let mut library = Library::default();
    library.record_play(&track(1), 1);
    library.clear_history();
    assert!(library.history().is_empty());
}

#[test]
fn only_touched_fields_are_marked_dirty() {
    let mut library = Library::default();
    library.set_favorite_track(&track(1), true, 1);
    assert_eq!(library.dirty_fields(), &[SyncField::Library]);
    library.record_play(&track(1), 2);
    assert_eq!(
        library.dirty_fields(),
        &[SyncField::Library, SyncField::History]
    );
    let flushed = library.take_dirty();
    assert_eq!(flushed.len(), 2);
    assert!(library.dirty_fields().is_empty());
}

#[test]
fn a_remote_refresh_does_not_discard_unflushed_local_changes() {
    let mut library = Library::default();
    library.set_favorite_track(&track(1), true, 1);
    library.merge_remote(SyncDocument {
        library: json!({ "tracks": {} }),
        history: json!([{ "id": 9, "timestamp": 5 }]),
        ..Default::default()
    });
    assert!(library.is_favorite(FavoriteKind::Track, "1"));
    assert_eq!(library.history()[0].id, 9);
}

#[test]
fn a_malformed_library_document_does_not_panic() {
    let mut library = library_with(json!("nonsense"));
    assert!(!library.is_favorite(FavoriteKind::Track, "1"));
    library.set_favorite_track(&track(1), true, 1);
    assert!(library.is_favorite(FavoriteKind::Track, "1"));
}

#[test]
fn string_ids_from_the_server_are_accepted() {
    let library = library_with(json!({
        "tracks": { "1": { "id": "1", "title": "Legacy", "duration": 100 } }
    }));
    let tracks = library.favorite_tracks();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, 1);
}
