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
