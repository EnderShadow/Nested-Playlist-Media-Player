use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use error_stack::{Report, IntoReport};
use thiserror::Error;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::ItemKey;
use lofty::tag::{Accessor};

#[derive(Error, Debug)]
pub enum LoadSaveError {
    #[error(transparent)]
    IOError(#[from] std::io::Error),
    #[error(transparent)]
    SerdeError(#[from] serde_json::Error)
}

#[derive(Error, Debug)]
pub enum LibraryError {
    #[error("Unknown playlist with UUID: {0}")]
    UnknownPlaylist(Uuid),
    #[error("Unknown song with UUID: {0}")]
    UnknownSong(Uuid),
    #[error("Loop detected in playlist with UUID: {0} and name: {1}")]
    PlaylistLoop(Uuid, String)
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_count: Option<u32>,
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
    pub contents: Vec<PlaylistEntry>,
    #[serde(skip)]
    dirty: bool
}

impl Playlist {
    pub fn new(name: String, description: String) -> Self {
        Playlist {
            uuid: Uuid::new_v4(),
            name,
            description,
            contents: Vec::new(),
            dirty: true
        }
    }

    fn add_entry(&mut self, entry: PlaylistEntry) {
        self.contents.push(entry);
        self.dirty = true;
    }

    fn insert_entry(&mut self, idx: usize, entry: PlaylistEntry) {
        self.contents.insert(idx, entry);
        self.dirty = true;
    }

    fn remove(&mut self, idx: usize) -> PlaylistEntry {
        self.dirty = true;
        self.contents.remove(idx)
    }

    fn remove_all(&mut self, uuid: Uuid) -> usize {
        // indices is reversed before being collected so that when it's iterated over to remove from self.contents, the indices of the elements to remove do not change
        let indices = self.contents.iter().enumerate().filter_map(|(idx, entry)| {
            if entry.uuid() == &uuid {
                Some(idx)
            } else {
                None
            }
        }).rev().collect::<Vec<_>>();
        indices.iter().for_each(|idx| {self.contents.remove(*idx);});
        if !indices.is_empty() {
            self.dirty = true;
        }
        indices.len()
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn mark_clean(&mut self) {
        self.dirty = false;
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

pub struct MediaLibrary {
    songs: HashMap<Uuid, AudioSource>,
    playlists: HashMap<Uuid, Playlist>,
    dirty: bool
}

impl MediaLibrary {
    fn new() -> Self {
        MediaLibrary {
            songs: HashMap::new(),
            playlists: HashMap::new(),
            dirty: false
        }
    }

    pub fn save(&mut self, config: Config) -> Result<(), Report<LoadSaveError>> {
        let data_dir = Path::new(&config.data_directory);
        if !data_dir.exists() {
            fs::create_dir_all(data_dir).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create data directory"))?;
        }

        let playlist_dir = data_dir.join("playlists");
        if !playlist_dir.exists() {
            fs::create_dir_all(&playlist_dir).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create playlist directory"))?;
        }

        if self.dirty {
            let library_file = data_dir.join("library.json");
            let new_library_file = library_file.with_added_extension("new");
            let mut file = File::create(&new_library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create library file"))?;
            // This should never fail
            let data = serde_json::to_string(&self.songs).expect("Failed to serialize library");
            file.write_all(data.as_bytes()).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to write library to file"))?;
            fs::rename(new_library_file, library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to overwrite old library file with new library file"))?;
            self.dirty = false;
        }

        for playlist in self.playlists.values_mut().filter(|p| p.dirty) {
            let playlist_file = playlist_dir.join(&playlist.name).with_added_extension("json");
            let temp_file = playlist_file.with_added_extension("new");
            let mut file = File::create(&temp_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create playlist file"))?;
            let data = serde_json::to_string(&playlist).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to serialize playlist file"))?;
            file.write_all(data.as_bytes()).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to write playlist file to file"))?;
            fs::rename(temp_file, playlist_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to overwrite playlist file with new playlist file"))?;
            playlist.mark_clean();
        }

        Ok(())
    }

    pub fn load(config: Config) -> Result<Self, Report<LoadSaveError>> {
        let mut library = Self::new();

        let data_dir = Path::new(&config.data_directory);
        if !data_dir.exists() {
            fs::create_dir_all(data_dir).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create data directory"))?;
        }

        let playlist_dir = data_dir.join("playlists");
        if !playlist_dir.exists() {
            fs::create_dir_all(&playlist_dir).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create playlist directory"))?;
        }

        let library_file = data_dir.join("library.json");
        if !library_file.exists() {
            let mut file = File::create_new(&library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create library file"))?;
            let _temp_vec: Vec<AudioSource> = Vec::new();
            // This should never fail
            let default_data = serde_json::to_string(&_temp_vec).expect("Failed to serialize default library");
            let result = file.write_all(default_data.as_bytes());
            if let Err(e) = result {
                fs::remove_file(library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to remove empty library file"))?;
                return Err(LoadSaveError::from(e).into_report().attach("Failed to write library to file"))
            }
        }

        library.load_songs(library_file)?;
        library.load_playlists(playlist_dir)?;

        Ok(library)
    }

    fn load_songs(&mut self, library_file: impl AsRef<Path>) -> Result<(), Report<LoadSaveError>> {
        // the library file should be accessible, so this `expect` should never fail
        let library_file = File::open(library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to open library file"))?;
        let songs: Vec<AudioSource> = serde_json::from_reader(library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to parse library"))?;
        songs.into_iter().for_each(|song| {self.songs.insert(song.uuid, song);});
        Ok(())
    }

    fn load_playlists(&mut self, playlist_dir: impl AsRef<Path>) -> Result<(), Report<LoadSaveError>> {
        let playlists: Vec<_> = playlist_dir.as_ref().read_dir().map_err(|e| LoadSaveError::from(e).into_report().attach("Could not read playlist directory"))?.filter_map(|playlist_file| {
            let file = playlist_file.ok()?;
            let file = File::open(file.path()).ok()?;
            let playlist: Option<Playlist> = serde_json::from_reader(file).ok();
            playlist
        }).collect();

        playlists.into_iter().for_each(|playlist| {self.playlists.insert(playlist.uuid, playlist);});

        for playlist in self.playlists.values() {
            if let Some(uuid_with_loop) = self.find_playlist_loop(&[], playlist) {
                todo!("A playlist loop has been detected, implement error handling for this")
            }

            let missing_playlists = self.find_missing_playlists(playlist);
            if !missing_playlists.is_empty() {
                todo!("A playlist references a non-existent playlist")
            }

            let missing_songs = self.find_missing_songs(playlist);
            if !missing_songs.is_empty() {
                todo!("A playlist references a non-existent song")
            }
        }

        Ok(())
    }

    fn find_playlist_loop(&self, seen_uuids: &[Uuid], playlist: &Playlist) -> Option<Uuid> {
        if seen_uuids.contains(&playlist.uuid) {
            Some(playlist.uuid)
        } else {
            let mut seen_uuids = Vec::from(seen_uuids);
            seen_uuids.push(playlist.uuid);
            playlist.contents.iter().find_map(|entry| {
                if let PlaylistEntry::Playlist(uuid) = entry {
                    let playlist = self.playlists.get(uuid);
                    if let Some(playlist) = playlist {
                        self.find_playlist_loop(&seen_uuids, playlist)
                    } else {
                        // playlist references a non-existent playlist, this is checked for in a different function
                        None
                    }
                } else {
                    // Don't need to check a playlist against a song
                    None
                }
            })
        }
    }

    fn find_missing_playlists<'a>(&self, playlist: &'a Playlist) -> Vec<&'a Uuid> {
        playlist.contents.iter().filter_map(|entry| {
            if let PlaylistEntry::Playlist(uuid) = entry && !self.playlists.contains_key(uuid) {
                Some(uuid)
            } else {
                None
            }
        }).collect()
    }

    fn find_missing_songs<'a>(&self, playlist: &'a Playlist) -> Vec<&'a Uuid> {
        playlist.contents.iter().filter_map(|entry| {
            if let PlaylistEntry::Song(uuid) = entry && !self.songs.contains_key(uuid) {
                Some(uuid)
            } else {
                None
            }
        }).collect()
    }

    pub fn add_song(&mut self, path: String) -> Uuid {
        let uuid = Uuid::new_v4();
        let song = if let Some(song) = read_song_metadata(&path, uuid) {
            song
        } else {
            // We failed to read the metadata, so use the file name as the title
            let title = Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            AudioSource {
                uuid,
                path,
                title,
                artist: None,
                album: None,
                genre: None,
                album_artist: None,
                track_number: None,
                track_count: None,
                year: None,
                duration: Default::default(),
            }
        };

        self.songs.insert(uuid, song);
        self.dirty = true;

        uuid
    }

    pub fn delete_song(&mut self, uuid: Uuid) -> bool {
        let audio_source = self.songs.remove(&uuid);
        if let Some(audio_source) = audio_source {
            let uuid = audio_source.uuid;
            self.playlists.iter_mut().for_each(|(_, playlist)| {
                playlist.remove_all(uuid);
            });
            self.dirty = true;

            true
        } else {
            false
        }
    }

    pub fn create_playlist(&mut self, name: String, description: String) -> Uuid {
        let playlist = Playlist::new(name, description);
        let uuid = playlist.uuid;
        self.playlists.insert(uuid, playlist);
        uuid
    }

    pub fn delete_playlist(&mut self, uuid: Uuid) -> bool{
        let playlist = self.playlists.remove(&uuid);
        if let Some(playlist) = playlist {
            let uuid = playlist.uuid;
            self.playlists.iter_mut().for_each(|(_, playlist)| {
                playlist.remove_all(uuid);
                playlist.mark_dirty();
            });

            true
        } else {
            false
        }
    }

    pub fn add_to_playlist(&mut self, entry: PlaylistEntry, playlist_uuid: Uuid) -> Result<(), Report<LibraryError>> {
        if let PlaylistEntry::Playlist(uuid_to_add) = entry {
            let playlist_to_add = self.playlists.get(&uuid_to_add).ok_or_else(|| Report::new(LibraryError::UnknownPlaylist(uuid_to_add)))?;
            if self.find_playlist_loop(&[playlist_uuid], playlist_to_add).is_some() {
                let playlist = self.playlists.get(&playlist_uuid).ok_or_else(|| Report::new(LibraryError::UnknownPlaylist(playlist_uuid)))?;
                return Err(Report::new(LibraryError::PlaylistLoop(playlist_uuid, playlist.name.clone())));
            }
        }

        let playlist = self.playlists.get_mut(&playlist_uuid).ok_or_else(|| Report::new(LibraryError::UnknownPlaylist(playlist_uuid)))?;
        playlist.add_entry(entry);
        playlist.mark_dirty();
        Ok(())
    }

    pub fn insert_into_playlist(&mut self, idx: usize, entry: PlaylistEntry, playlist_uuid: Uuid) -> Result<(), Report<LibraryError>> {
        if let PlaylistEntry::Playlist(uuid_to_add) = entry {
            let playlist_to_add = self.playlists.get(&uuid_to_add).ok_or_else(|| Report::new(LibraryError::UnknownPlaylist(uuid_to_add)))?;
            if self.find_playlist_loop(&[playlist_uuid], playlist_to_add).is_some() {
                let playlist = self.playlists.get(&playlist_uuid).ok_or_else(|| Report::new(LibraryError::UnknownPlaylist(playlist_uuid)))?;
                return Err(Report::new(LibraryError::PlaylistLoop(playlist_uuid, playlist.name.clone())));
            }
        }

        let playlist = self.playlists.get_mut(&playlist_uuid).ok_or_else(|| Report::new(LibraryError::UnknownPlaylist(playlist_uuid)))?;
        playlist.insert_entry(idx, entry);
        playlist.mark_dirty();
        Ok(())
    }

    pub fn remove_from_playlist(&mut self, idx: usize, playlist_uuid: Uuid) -> Result<PlaylistEntry, Report<LibraryError>> {
        if let Some(playlist) = self.playlists.get_mut(&playlist_uuid) {
            let entry = playlist.remove(idx);
            playlist.mark_dirty();
            Ok(entry)
        } else {
            Err(Report::new(LibraryError::UnknownPlaylist(playlist_uuid)))
        }
    }
}

fn read_song_metadata(path: impl AsRef<Path> + ToString, uuid: Uuid) -> Option<AudioSource> {
    // try to guess file type from extension. If that fails, try to guess from file contents
    let file = if let Ok(file) = lofty::read_from_path(&path) {
        file
    } else {
        lofty::probe::Probe::open(&path).ok()?.guess_file_type().ok()?.read().ok()?
    };
    let tag = file.first_tag()?;
    let title = if let Some(title) = tag.title() {
        title.to_string()
    } else {
        path.as_ref().file_name()?.to_str()?.to_string()
    };
    Some(AudioSource {
        uuid,
        path: path.to_string(),
        title,
        artist: tag.artist().map(String::from),
        album: tag.album().map(String::from),
        genre: tag.genre().map(String::from),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(String::from),
        track_number: tag.track(),
        track_count: tag.track_total(),
        year: tag.date().map(|d| d.year.to_string()),
        duration: file.properties().duration(),
    })
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub data_directory: String
}