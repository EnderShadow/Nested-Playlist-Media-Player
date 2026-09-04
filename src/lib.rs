use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;
use serde::{Serialize, Deserialize, Deserializer};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSource {
    pub uuid: Uuid,
    #[serde(alias = "uri")]
    pub path: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    #[serde(deserialize_with = "ok_or_default")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_count: Option<NonZeroU32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    #[serde(with = "serde_millis")]
    pub duration: Duration,
}

#[derive(Serialize, Deserialize)]
pub struct Playlist {
    pub uuid: Uuid,
    pub name: String,
    pub description: String,
    #[serde(alias="sources")]
    pub contents: Vec<PlaylistEntry>
}

impl Playlist {
    pub fn new(name: String, description: String) -> Self {
        Playlist {
            uuid: Uuid::new_v4(),
            name,
            description,
            contents: Vec::new()
        }
    }

    pub fn add_song(&mut self, uuid: Uuid) {
        self.contents.push(PlaylistEntry::Song(uuid))
    }

    pub fn insert_song(&mut self, idx: usize, uuid: Uuid) {
        self.contents.insert(idx, PlaylistEntry::Song(uuid))
    }

    pub fn add_playlist(&mut self, uuid: Uuid) {
        self.contents.push(PlaylistEntry::Playlist(uuid))
    }

    pub fn insert_playlist(&mut self, idx: usize, uuid: Uuid) {
        self.contents.insert(idx, PlaylistEntry::Playlist(uuid))
    }

    pub fn remove(&mut self, idx: usize) -> PlaylistEntry {
        self.contents.remove(idx)
    }

    pub fn remove_all(&mut self, uuid: Uuid) -> usize {
        // indices is reversed before being collected so that when it's iterated over to remove from self.contents, the indices of the elements to remove do not change
        let indices = self.contents.iter().enumerate().filter_map(|(idx, entry)| {
            if entry.uuid() == &uuid {
                Some(idx)
            } else {
                None
            }
        }).rev().collect::<Vec<_>>();
        indices.iter().for_each(|idx| {self.contents.remove(*idx);});
        indices.len()
    }

    pub fn flat_len(&self, library: &MediaLibrary) -> usize {
        self.contents.iter().map(|entry| {
            match entry {
                PlaylistEntry::Playlist(id) => library.playlists[id].flat_len(library),
                PlaylistEntry::Song(_) => 1
            }
        }).sum()
    }
}

#[derive(Serialize, Deserialize, Copy, Clone)]
#[serde(tag = "type", content = "uuid", rename_all = "lowercase")]
pub enum PlaylistEntry {
    Song(Uuid),
    Playlist(Uuid)
}

impl PlaylistEntry {
    pub fn uuid(&self) -> &Uuid {
        match self {
            PlaylistEntry::Song(id) => id,
            PlaylistEntry::Playlist(id) => id
        }
    }
}

fn ok_or_default<'a, T, D>(deserializer: D) -> Result<T, D::Error> where T: Deserialize<'a> + Default, D: Deserializer<'a>
{
    let v: Value = Deserialize::deserialize(deserializer)?;
    Ok(T::deserialize(v).unwrap_or_default())
}

pub struct MediaLibrary {
    pub songs: HashMap<Uuid, AudioSource>,
    pub playlists: HashMap<Uuid, Playlist>
}

impl MediaLibrary {
    fn new() -> Self {
        MediaLibrary {
            songs: HashMap::new(),
            playlists: HashMap::new()
        }
    }

    pub fn load(config: Config) -> Self {
        let mut library = Self::new();

        let data_dir = Path::new(&config.data_directory);
        if !data_dir.exists() {
            fs::create_dir_all(data_dir).expect("Could not create data directory");
        }

        let playlist_dir = data_dir.join("playlists");
        if !playlist_dir.exists() {
            fs::create_dir_all(&playlist_dir).expect("Could not create playlist directory");
        }

        let library_file = data_dir.join("library.json");
        if !library_file.exists() {
            let mut file = File::create_new(&library_file).expect("Could not create library file");
            let _temp_vec: Vec<AudioSource> = Vec::new();
            let default_data = serde_json::to_string(&_temp_vec).expect("Could not serialize empty vec");
            let result = file.write_all(default_data.as_bytes());
            if result.is_err() {
                fs::remove_file(library_file).expect("Could not remove empty library file");
                panic!("Could not write default library to file");
            }
        }

        library.load_songs(library_file);
        library.load_playlists(playlist_dir);

        library
    }

    fn load_songs(&mut self, library_file: impl AsRef<Path>) {
        // the library file should be accessible, so this `expect` should never fail
        let library_file = File::open(library_file).expect("Could not open library file");
        let songs: Vec<AudioSource> = serde_json::from_reader(library_file).expect("Failed to parse library");
        songs.into_iter().for_each(|song| {self.songs.insert(song.uuid, song);});
    }

    fn load_playlists(&mut self, playlist_dir: impl AsRef<Path>) {
        let playlists: Vec<_> = playlist_dir.as_ref().read_dir().expect("Could not read playlist directory").filter_map(|playlist_file| {
            let file = playlist_file.ok()?;
            let file = File::open(file.path()).ok()?;
            let playlist: Option<Playlist> = serde_json::from_reader(file).ok();
            playlist
        }).collect();

        playlists.into_iter().for_each(|playlist| {self.playlists.insert(playlist.uuid, playlist);});

        for (_, playlist) in &self.playlists {
            if self.check_contains_loop(&vec![], playlist) {
                todo!("A playlist loop has been detected, implement error handling for this")
            }

            if self.contains_missing_playlist(playlist) {
                todo!("A playlist references a non-existent playlist")
            }

            if self.contains_missing_song(playlist) {
                todo!("A playlist references a non-existent song")
            }
        }
    }

    fn check_contains_loop(&self, seen_uuids: &Vec<Uuid>, playlist: &Playlist) -> bool {
        if seen_uuids.contains(&playlist.uuid) {
            true
        } else {
            let mut seen_uuids = seen_uuids.clone();
            seen_uuids.push(playlist.uuid);
            playlist.contents.iter().any(|entry| {
                if let PlaylistEntry::Playlist(uuid) = entry {
                    let playlist = self.playlists.get(uuid);
                    if let Some(playlist) = playlist {
                        self.check_contains_loop(&seen_uuids, playlist)
                    } else {
                        // playlist references a non-existent playlist, this will be handled after loops are checked
                        false
                    }
                } else {
                    // Don't need to check a playlist against a song
                    false
                }
            })
        }
    }

    fn contains_missing_playlist(&self, playlist: &Playlist) -> bool {
        for entry in playlist.contents.iter() {
            if let PlaylistEntry::Playlist(uuid) = entry {
                if !self.playlists.contains_key(uuid) {
                    return true
                }
            }
        }
        false
    }

    fn contains_missing_song(&self, playlist: &Playlist) -> bool {
        for entry in playlist.contents.iter() {
            if let PlaylistEntry::Song(uuid) = entry {
                if !self.songs.contains_key(uuid) {
                    return true
                }
            }
        }
        false
    }

    pub fn create_playlist(&mut self, name: String, description: String) -> Uuid {
        let playlist = Playlist::new(name, description);
        let uuid = playlist.uuid;
        self.playlists.insert(uuid, playlist);
        uuid
    }

    pub fn remove_playlist(&mut self, uuid: Uuid) -> bool{
        let playlist = self.playlists.remove(&uuid);
        if let Some(playlist) = playlist {
            let uuid = playlist.uuid;
            self.playlists.iter_mut().for_each(|(_, playlist)| {
                playlist.remove_all(uuid);
            });

            true
        } else {
            false
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub data_directory: String
}