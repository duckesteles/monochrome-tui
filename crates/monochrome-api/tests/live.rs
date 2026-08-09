use monochrome_api::Catalog;

#[tokio::test]
#[ignore = "reaches the public monochrome catalog; run with --ignored"]
async fn the_live_catalog_answers_a_search_and_an_album_lookup() {
    let catalog = Catalog::with_defaults().expect("catalog");

    let tracks = catalog.search_tracks("daft punk").await.expect("search");
    assert!(!tracks.is_empty(), "the catalog returned no tracks");
    assert!(
        tracks
            .iter()
            .any(|track| track.artist_name() == "Daft Punk")
    );

    let album = catalog.album(1550545).await.expect("album");
    assert_eq!(album.title, "Discovery");
    assert_eq!(album.tracks.len(), 14);

    let artist = catalog.artist(8847).await.expect("artist");
    assert_eq!(artist.name, "Daft Punk");

    let instance = catalog.active_instance().expect("an instance answered");
    println!("answered by {}", instance.url);
}

#[tokio::test]
#[ignore = "reaches the public monochrome catalog; run with --ignored"]
async fn the_live_catalog_hands_over_a_whole_playlist() {
    let catalog = Catalog::with_defaults().expect("catalog");

    let (playlist, tracks) = catalog
        .playlist("19bbde7f-1fa0-4822-b285-fcbce44a18c4")
        .await
        .expect("playlist");
    assert_eq!(playlist.title, "Hip-Hop Party Hits");

    let promised = playlist.number_of_tracks.expect("a track count") as usize;
    assert!(promised > 100, "pick a playlist longer than one page");
    assert_eq!(
        tracks.len(),
        promised,
        "the playlist says {promised} tracks but only {} arrived",
        tracks.len()
    );
    println!(
        "{} tracks over {} pages",
        tracks.len(),
        promised.div_ceil(100)
    );
}

#[tokio::test]
#[ignore = "reaches the public monochrome catalog; run with --ignored"]
async fn the_live_catalog_answers_a_recommendation() {
    let catalog = Catalog::with_defaults().expect("catalog");
    let tracks = catalog.recommendations(423283215).await.expect("radio");
    assert!(
        !tracks.is_empty(),
        "a version filter shut every instance out"
    );
    println!(
        "{} recommendations, first is {}",
        tracks.len(),
        tracks[0].title
    );
}
