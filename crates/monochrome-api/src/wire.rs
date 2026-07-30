use monochrome_core::model::{Album, AlbumRef, Artist, ArtistRef, Playlist, Quality, Track};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Page<T> {
    #[serde(default)]
    pub items: Vec<T>,
    #[serde(default, rename = "totalNumberOfItems")]
    pub total: u32,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct SearchSections {
    #[serde(default)]
    pub artists: Page<WireArtist>,
    #[serde(default)]
    pub albums: Page<WireAlbum>,
    #[serde(default)]
    pub playlists: Page<WirePlaylist>,
    #[serde(default)]
    pub tracks: Page<WireTrack>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistEnvelope {
    pub artist: WireArtist,
}

#[derive(Debug, Deserialize)]
pub struct ArtistAlbumsEnvelope {
    #[serde(default)]
    pub albums: Page<WireAlbum>,
    #[serde(default)]
    pub tracks: Vec<WireTrack>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireArtistRef {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireAlbumRef {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default, rename = "releaseDate")]
    pub release_date: Option<String>,
    #[serde(default, rename = "numberOfTracks")]
    pub number_of_tracks: Option<u32>,
    #[serde(default)]
    pub artist: Option<WireArtistRef>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireTrack {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default, rename = "trackNumber")]
    pub track_number: Option<u32>,
    #[serde(default, rename = "volumeNumber")]
    pub volume_number: Option<u32>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, rename = "audioQuality")]
    pub audio_quality: Option<String>,
    #[serde(default, rename = "replayGain")]
    pub replay_gain: Option<f32>,
    #[serde(default)]
    pub peak: Option<f32>,
    #[serde(default = "yes", rename = "streamReady")]
    pub stream_ready: bool,
    #[serde(default = "yes", rename = "allowStreaming")]
    pub allow_streaming: bool,
    #[serde(default)]
    pub artist: Option<WireArtistRef>,
    #[serde(default)]
    pub artists: Vec<WireArtistRef>,
    #[serde(default)]
    pub album: Option<WireAlbumRef>,
    #[serde(default, rename = "mediaMetadata")]
    pub media_metadata: Option<MediaMetadata>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MediaMetadata {
    #[serde(default)]
    pub tags: Vec<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct WireAlbumItem {
    pub item: WireTrack,
}

#[derive(Debug, Deserialize)]
pub struct WireAlbum {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default, rename = "numberOfTracks")]
    pub number_of_tracks: Option<u32>,
    #[serde(default, rename = "releaseDate")]
    pub release_date: Option<String>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default, rename = "type")]
    pub album_type: Option<String>,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default, rename = "audioQuality")]
    pub audio_quality: Option<String>,
    #[serde(default)]
    pub artist: Option<WireArtistRef>,
    #[serde(default)]
    pub artists: Vec<WireArtistRef>,
    #[serde(default)]
    pub items: Vec<WireAlbumItem>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireArtist {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub popularity: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct WireCreator {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WirePlaylist {
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "numberOfTracks")]
    pub number_of_tracks: Option<u32>,
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default, rename = "squareImage")]
    pub square_image: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub creator: Option<WireCreator>,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistEnvelope {
    #[serde(flatten)]
    pub playlist: WirePlaylist,
    #[serde(default)]
    pub items: Vec<WireAlbumItem>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestEnvelope {
    pub data: ManifestResource,
}

#[derive(Debug, Deserialize)]
pub struct ManifestResource {
    pub attributes: ManifestAttributes,
}

#[derive(Debug, Deserialize)]
pub struct ManifestAttributes {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default, rename = "trackPresentation")]
    pub presentation: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
}

fn quality_of(tags: Option<&MediaMetadata>, fallback: Option<&str>) -> Quality {
    if let Some(metadata) = tags {
        if metadata.tags.iter().any(|t| t == "HIRES_LOSSLESS") {
            return Quality::HiRes;
        }
        if metadata.tags.iter().any(|t| t == "DOLBY_ATMOS") {
            return Quality::Atmos;
        }
    }
    fallback
        .and_then(Quality::parse)
        .unwrap_or(Quality::Lossless)
}

impl WireArtistRef {
    pub fn into_core(self) -> ArtistRef {
        ArtistRef {
            id: self.id,
            name: self.name.unwrap_or_else(|| "Unknown Artist".into()),
            picture: self.picture,
        }
    }
}

impl WireAlbumRef {
    pub fn into_core(self) -> AlbumRef {
        AlbumRef {
            id: self.id,
            title: self.title.unwrap_or_default(),
            cover: self.cover,
            release_date: self.release_date,
            artist: self.artist.map(WireArtistRef::into_core),
            number_of_tracks: self.number_of_tracks,
        }
    }
}

impl WireTrack {
    pub fn into_core(self) -> Track {
        let quality = quality_of(self.media_metadata.as_ref(), self.audio_quality.as_deref());
        Track {
            id: self.id,
            title: self.title.unwrap_or_else(|| "Unknown".into()),
            duration: self.duration,
            explicit: self.explicit,
            artist: self.artist.map(WireArtistRef::into_core),
            artists: self
                .artists
                .into_iter()
                .map(WireArtistRef::into_core)
                .collect(),
            album: self.album.map(WireAlbumRef::into_core),
            isrc: self.isrc,
            track_number: self.track_number,
            volume_number: self.volume_number,
            copyright: self.copyright,
            version: self.version,
            quality,
            replay_gain: self.replay_gain,
            peak: self.peak,
            stream_ready: self.stream_ready && self.allow_streaming,
        }
    }
}

impl WireAlbum {
    pub fn into_core(self) -> Album {
        let artist = self
            .artist
            .or_else(|| self.artists.first().cloned())
            .map(WireArtistRef::into_core);
        Album {
            id: self.id,
            title: self.title.unwrap_or_else(|| "Unknown".into()),
            cover: self.cover,
            release_date: self.release_date,
            artist,
            number_of_tracks: self.number_of_tracks,
            duration: self.duration,
            explicit: self.explicit,
            quality: self
                .audio_quality
                .as_deref()
                .and_then(Quality::parse)
                .unwrap_or(Quality::Lossless),
            album_type: self.album_type,
            copyright: self.copyright,
            tracks: self
                .items
                .into_iter()
                .map(|entry| entry.item.into_core())
                .collect(),
        }
    }
}

impl WireArtist {
    pub fn into_core(self) -> Artist {
        Artist {
            id: self.id,
            name: self.name.unwrap_or_else(|| "Unknown Artist".into()),
            picture: self.picture,
            popularity: self.popularity,
        }
    }
}

impl WirePlaylist {
    pub fn into_core(self) -> Option<Playlist> {
        Some(Playlist {
            uuid: self.uuid?,
            title: self.title.unwrap_or_else(|| "Untitled".into()),
            image: self.image.or(self.square_image),
            number_of_tracks: self.number_of_tracks,
            duration: self.duration,
            creator_name: self.creator.and_then(|c| c.name),
            description: self.description,
        })
    }
}
