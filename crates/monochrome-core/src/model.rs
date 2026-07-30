use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Quality {
    Low,
    High,
    #[default]
    Lossless,
    HiRes,
    Atmos,
}

impl Quality {
    pub fn as_tidal(self) -> &'static str {
        match self {
            Quality::Low => "LOW",
            Quality::High => "HIGH",
            Quality::Lossless => "LOSSLESS",
            Quality::HiRes => "HI_RES_LOSSLESS",
            Quality::Atmos => "DOLBY_ATMOS",
        }
    }

    pub fn as_amazon(self) -> &'static str {
        match self {
            Quality::Low => "SD_LOW",
            Quality::High => "SD_HIGH",
            Quality::Lossless => "HD",
            Quality::HiRes | Quality::Atmos => "UHD",
        }
    }

    pub fn as_deezer(self) -> &'static str {
        match self {
            Quality::Low | Quality::High => "MP3_320",
            Quality::Lossless | Quality::HiRes | Quality::Atmos => "FLAC",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "LOW" => Some(Quality::Low),
            "HIGH" => Some(Quality::High),
            "LOSSLESS" => Some(Quality::Lossless),
            "HI_RES" | "HI_RES_LOSSLESS" => Some(Quality::HiRes),
            "DOLBY_ATMOS" => Some(Quality::Atmos),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::High => "high",
            Quality::Lossless => "lossless",
            Quality::HiRes => "hi-res",
            Quality::Atmos => "atmos",
        }
    }

    pub const ALL: [Quality; 4] = [
        Quality::Low,
        Quality::High,
        Quality::Lossless,
        Quality::HiRes,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub artist: Option<ArtistRef>,
    #[serde(default)]
    pub number_of_tracks: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub duration: u32,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub artist: Option<ArtistRef>,
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub album: Option<AlbumRef>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub volume_number: Option<u32>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub quality: Quality,
    #[serde(default)]
    pub replay_gain: Option<f32>,
    #[serde(default)]
    pub peak: Option<f32>,
    #[serde(default = "default_true")]
    pub stream_ready: bool,
}

fn default_true() -> bool {
    true
}

impl Track {
    pub fn artist_name(&self) -> &str {
        self.artist
            .as_ref()
            .or_else(|| self.artists.first())
            .map(|a| a.name.as_str())
            .unwrap_or("Unknown Artist")
    }

    pub fn album_title(&self) -> &str {
        self.album.as_ref().map(|a| a.title.as_str()).unwrap_or("")
    }

    pub fn display_title(&self) -> String {
        match self.version.as_deref().filter(|v| !v.is_empty()) {
            Some(version) => format!("{} ({})", self.title, version),
            None => self.title.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub artist: Option<ArtistRef>,
    #[serde(default)]
    pub number_of_tracks: Option<u32>,
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub quality: Quality,
    #[serde(default)]
    pub album_type: Option<String>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

impl Album {
    pub fn year(&self) -> Option<&str> {
        self.release_date.as_deref().and_then(|d| d.get(0..4))
    }

    pub fn artist_name(&self) -> &str {
        self.artist
            .as_ref()
            .map(|a| a.name.as_str())
            .unwrap_or("Unknown Artist")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub popularity: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub uuid: String,
    pub title: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub number_of_tracks: Option<u32>,
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub creator_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Serialize, Deserialize)]
pub enum FavoriteKind {
    Track,
    Album,
    Artist,
    Playlist,
}

impl FavoriteKind {
    pub fn plural(self) -> &'static str {
        match self {
            FavoriteKind::Track => "tracks",
            FavoriteKind::Album => "albums",
            FavoriteKind::Artist => "artists",
            FavoriteKind::Playlist => "playlists",
        }
    }
}
