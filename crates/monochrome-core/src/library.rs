use crate::model::{Album, AlbumRef, Artist, ArtistRef, FavoriteKind, Playlist, Track};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const HISTORY_LIMIT: usize = 100;
pub const HISTORY_THRESHOLD_SECS: u32 = 10;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncDocument {
    #[serde(default, rename = "appUserId")]
    pub app_user_id: Option<String>,
    #[serde(default)]
    pub profile: Value,
    #[serde(default)]
    pub library: Value,
    #[serde(default)]
    pub history: Value,
    #[serde(default, rename = "userPlaylists")]
    pub user_playlists: Value,
    #[serde(default, rename = "userFolders")]
    pub user_folders: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncField {
    Library,
    History,
    UserPlaylists,
    UserFolders,
}

impl SyncField {
    pub fn wire_name(self) -> &'static str {
        match self {
            SyncField::Library => "library",
            SyncField::History => "history",
            SyncField::UserPlaylists => "userPlaylists",
            SyncField::UserFolders => "userFolders",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    document: SyncDocument,
    dirty: Vec<SyncField>,
}

impl Library {
    pub fn new(document: SyncDocument) -> Self {
        Self {
            document,
            dirty: Vec::new(),
        }
    }

    pub fn document(&self) -> &SyncDocument {
        &self.document
    }

    pub fn dirty_fields(&self) -> &[SyncField] {
        &self.dirty
    }

    pub fn take_dirty(&mut self) -> Vec<(SyncField, Value)> {
        let fields = std::mem::take(&mut self.dirty);
        fields
            .into_iter()
            .map(|field| {
                let value = match field {
                    SyncField::Library => self.document.library.clone(),
                    SyncField::History => self.document.history.clone(),
                    SyncField::UserPlaylists => self.document.user_playlists.clone(),
                    SyncField::UserFolders => self.document.user_folders.clone(),
                };
                (field, value)
            })
            .collect()
    }

    pub fn merge_remote(&mut self, document: SyncDocument) {
        let pending = self.dirty.clone();
        let previous = self.document.clone();
        self.document = document;
        for field in &pending {
            match field {
                SyncField::Library => self.document.library = previous.library.clone(),
                SyncField::History => self.document.history = previous.history.clone(),
                SyncField::UserPlaylists => {
                    self.document.user_playlists = previous.user_playlists.clone()
                }
                SyncField::UserFolders => {
                    self.document.user_folders = previous.user_folders.clone()
                }
            }
        }
    }

    fn mark(&mut self, field: SyncField) {
        if !self.dirty.contains(&field) {
            self.dirty.push(field);
        }
    }

    fn section(&self, kind: FavoriteKind) -> Option<&Map<String, Value>> {
        self.document
            .library
            .as_object()?
            .get(kind.plural())?
            .as_object()
    }

    pub fn is_favorite(&self, kind: FavoriteKind, key: &str) -> bool {
        self.section(kind)
            .map(|section| section.contains_key(key))
            .unwrap_or(false)
    }

    pub fn favorite_count(&self, kind: FavoriteKind) -> usize {
        self.section(kind).map(|s| s.len()).unwrap_or(0)
    }

    fn set_favorite_raw(&mut self, kind: FavoriteKind, key: &str, entry: Option<Value>) {
        if !self.document.library.is_object() {
            self.document.library = Value::Object(Map::new());
        }
        let library = self.document.library.as_object_mut().expect("object");
        let section = library
            .entry(kind.plural().to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !section.is_object() {
            *section = Value::Object(Map::new());
        }
        let section = section.as_object_mut().expect("object");
        match entry {
            Some(value) => {
                section.insert(key.to_string(), value);
            }
            None => {
                section.remove(key);
            }
        }
        self.mark(SyncField::Library);
    }

    pub fn set_favorite_track(&mut self, track: &Track, added: bool, now_ms: u64) {
        let key = track.id.to_string();
        let entry = added.then(|| minify_track(track, now_ms));
        self.set_favorite_raw(FavoriteKind::Track, &key, entry);
    }

    pub fn set_favorite_album(&mut self, album: &Album, added: bool, now_ms: u64) {
        let key = album.id.to_string();
        let entry = added.then(|| minify_album(album, now_ms));
        self.set_favorite_raw(FavoriteKind::Album, &key, entry);
    }

    pub fn set_favorite_artist(&mut self, artist: &Artist, added: bool, now_ms: u64) {
        let key = artist.id.to_string();
        let entry = added.then(|| minify_artist(artist, now_ms));
        self.set_favorite_raw(FavoriteKind::Artist, &key, entry);
    }

    pub fn set_favorite_playlist(&mut self, playlist: &Playlist, added: bool, now_ms: u64) {
        let key = playlist.uuid.clone();
        let entry = added.then(|| minify_playlist(playlist, now_ms));
        self.set_favorite_raw(FavoriteKind::Playlist, &key, entry);
    }

    fn sorted_entries(&self, kind: FavoriteKind) -> Vec<&Value> {
        let Some(section) = self.section(kind) else {
            return Vec::new();
        };
        let mut entries: Vec<&Value> = section.values().collect();
        entries.sort_by_key(|entry| {
            std::cmp::Reverse(entry.get("addedAt").and_then(Value::as_u64).unwrap_or(0))
        });
        entries
    }

    pub fn favorite_tracks(&self) -> Vec<Track> {
        self.sorted_entries(FavoriteKind::Track)
            .into_iter()
            .filter_map(track_from_value)
            .collect()
    }

    pub fn favorite_albums(&self) -> Vec<Album> {
        self.sorted_entries(FavoriteKind::Album)
            .into_iter()
            .filter_map(album_from_value)
            .collect()
    }

    pub fn favorite_artists(&self) -> Vec<Artist> {
        self.sorted_entries(FavoriteKind::Artist)
            .into_iter()
            .filter_map(artist_from_value)
            .collect()
    }

    pub fn favorite_playlists(&self) -> Vec<Playlist> {
        self.sorted_entries(FavoriteKind::Playlist)
            .into_iter()
            .filter_map(playlist_from_value)
            .collect()
    }

    pub fn section_names(&self) -> Vec<String> {
        self.document
            .library
            .as_object()
            .map(|sections| {
                sections
                    .iter()
                    .map(|(name, value)| {
                        let count = value.as_object().map(|entries| entries.len()).unwrap_or(0);
                        format!("{name}: {count}")
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn history(&self) -> Vec<Track> {
        self.document
            .history
            .as_array()
            .map(|entries| entries.iter().filter_map(track_from_value).collect())
            .unwrap_or_default()
    }

    pub fn record_play(&mut self, track: &Track, now_ms: u64) {
        let mut entry = minify_track(track, now_ms);
        let mut timestamp = now_ms;
        let mut entries: Vec<Value> = self
            .document
            .history
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|value| value.get("id").and_then(json_id) != Some(track.id))
            .collect();
        if let Some(latest) = entries
            .first()
            .and_then(|value| value.get("timestamp"))
            .and_then(Value::as_u64)
            && latest >= timestamp
        {
            timestamp = latest + 1;
        }
        if let Some(object) = entry.as_object_mut() {
            object.insert("timestamp".into(), json!(timestamp));
        }
        entries.insert(0, entry);
        entries.truncate(HISTORY_LIMIT);
        self.document.history = Value::Array(entries);
        self.mark(SyncField::History);
    }

    pub fn clear_history(&mut self) {
        self.document.history = Value::Array(Vec::new());
        self.mark(SyncField::History);
    }
}

fn json_id(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn minify_artist_ref(artist: &ArtistRef) -> Value {
    json!({ "id": artist.id, "name": artist.name })
}

fn minify_album_ref(album: &AlbumRef) -> Value {
    json!({
        "id": album.id,
        "title": album.title,
        "cover": album.cover,
        "releaseDate": album.release_date,
        "artist": album.artist.as_ref().map(minify_artist_ref),
        "numberOfTracks": album.number_of_tracks,
    })
}

pub fn minify_track(track: &Track, now_ms: u64) -> Value {
    json!({
        "id": track.id,
        "addedAt": now_ms,
        "title": track.title,
        "duration": track.duration,
        "explicit": track.explicit,
        "artist": track
            .artist
            .as_ref()
            .or_else(|| track.artists.first())
            .map(minify_artist_ref),
        "artists": track.artists.iter().map(minify_artist_ref).collect::<Vec<_>>(),
        "album": track.album.as_ref().map(minify_album_ref),
        "copyright": track.copyright,
        "isrc": track.isrc,
        "trackNumber": track.track_number,
        "version": track.version,
    })
}

pub fn minify_album(album: &Album, now_ms: u64) -> Value {
    json!({
        "id": album.id,
        "addedAt": now_ms,
        "title": album.title,
        "cover": album.cover,
        "releaseDate": album.release_date,
        "explicit": album.explicit,
        "artist": album.artist.as_ref().map(minify_artist_ref),
        "type": album.album_type,
        "numberOfTracks": album.number_of_tracks,
    })
}

pub fn minify_artist(artist: &Artist, now_ms: u64) -> Value {
    json!({
        "id": artist.id,
        "addedAt": now_ms,
        "name": artist.name,
        "picture": artist.picture,
    })
}

pub fn minify_playlist(playlist: &Playlist, now_ms: u64) -> Value {
    json!({
        "uuid": playlist.uuid,
        "addedAt": now_ms,
        "title": playlist.title,
        "image": playlist.image,
        "numberOfTracks": playlist.number_of_tracks.unwrap_or(0),
        "user": playlist.creator_name.as_ref().map(|name| json!({ "name": name })),
    })
}

fn artist_ref_from_value(value: &Value) -> Option<ArtistRef> {
    Some(ArtistRef {
        id: value.get("id").and_then(json_id)?,
        name: value.get("name").and_then(Value::as_str)?.to_string(),
        picture: value
            .get("picture")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn album_ref_from_value(value: &Value) -> Option<AlbumRef> {
    Some(AlbumRef {
        id: value.get("id").and_then(json_id)?,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        cover: value
            .get("cover")
            .and_then(Value::as_str)
            .map(str::to_string),
        release_date: value
            .get("releaseDate")
            .and_then(Value::as_str)
            .map(str::to_string),
        artist: value.get("artist").and_then(artist_ref_from_value),
        number_of_tracks: value
            .get("numberOfTracks")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
    })
}

pub fn track_from_value(value: &Value) -> Option<Track> {
    let id = value.get("id").and_then(json_id)?;
    Some(Track {
        id,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        duration: value
            .get("duration")
            .and_then(Value::as_u64)
            .unwrap_or_default() as u32,
        explicit: value
            .get("explicit")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        artist: value.get("artist").and_then(artist_ref_from_value),
        artists: value
            .get("artists")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(artist_ref_from_value).collect())
            .unwrap_or_default(),
        album: value.get("album").and_then(album_ref_from_value),
        isrc: value
            .get("isrc")
            .and_then(Value::as_str)
            .map(str::to_string),
        track_number: value
            .get("trackNumber")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        volume_number: None,
        copyright: value
            .get("copyright")
            .and_then(Value::as_str)
            .map(str::to_string),
        version: value
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
        quality: Default::default(),
        replay_gain: None,
        peak: None,
        stream_ready: true,
    })
}

pub fn album_from_value(value: &Value) -> Option<Album> {
    Some(Album {
        id: value.get("id").and_then(json_id)?,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        cover: value
            .get("cover")
            .and_then(Value::as_str)
            .map(str::to_string),
        release_date: value
            .get("releaseDate")
            .and_then(Value::as_str)
            .map(str::to_string),
        artist: value.get("artist").and_then(artist_ref_from_value),
        number_of_tracks: value
            .get("numberOfTracks")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        duration: None,
        explicit: value
            .get("explicit")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        quality: Default::default(),
        album_type: value
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string),
        copyright: None,
        tracks: Vec::new(),
    })
}

pub fn artist_from_value(value: &Value) -> Option<Artist> {
    Some(Artist {
        id: value.get("id").and_then(json_id)?,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        picture: value
            .get("picture")
            .and_then(Value::as_str)
            .map(str::to_string),
        popularity: None,
    })
}

pub fn playlist_from_value(value: &Value) -> Option<Playlist> {
    let uuid = value
        .get("uuid")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)?
        .to_string();
    Some(Playlist {
        uuid,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_string(),
        image: value
            .get("image")
            .and_then(Value::as_str)
            .map(str::to_string),
        number_of_tracks: value
            .get("numberOfTracks")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        duration: value
            .get("duration")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        creator_name: value
            .get("user")
            .and_then(|u| u.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        description: None,
    })
}
