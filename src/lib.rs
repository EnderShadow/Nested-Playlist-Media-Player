use error_stack::{IntoReport, Report};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTag};
use symphonia::default::get_probe;
use thiserror::Error;
use uuid::Uuid;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

#[derive(Error, Debug)]
pub enum LoadSaveError {
    #[error(transparent)]
    IOError(#[from] std::io::Error),
    #[error(transparent)]
    SerdeError(#[from] serde_json::Error),
    #[error(transparent)]
    ZipError(#[from] zip::result::ZipError),
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct ImportSummary {
    pub songs: usize,
    pub playlists: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    format_version: u32,
}

#[derive(Error, Debug)]
pub enum LibraryError {
    #[error("Unknown playlist with UUID: {0}")]
    UnknownPlaylist(Uuid),
    #[error("Unknown song with UUID: {0}")]
    UnknownSong(Uuid),
    #[error("Loop detected in playlist with UUID: {0} and name: {1}")]
    PlaylistLoop(Uuid, String),
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioSource {
    pub uuid: Uuid,
    #[serde(alias = "uri")]
    pub path: PathBuf,
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
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "serde_millis::serialize", deserialize_with = "deserialize_duration")]
    pub duration: Option<Duration>,
}

fn deserialize_duration<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Duration>, D::Error> {
    serde_millis::deserialize(deserializer).map(|duration: Option<Duration>| duration.filter(|duration| !duration.is_zero()))
}

#[derive(Clone, Debug)]
pub struct PlaylistInfo {
    pub uuid: Uuid,
    pub name: String,
    pub description: String,
}

#[derive(Serialize, Deserialize)]
pub struct Playlist {
    uuid: Uuid,
    name: String,
    description: String,
    #[serde(alias = "sources")]
    contents: Vec<PlaylistEntry>,
    #[serde(skip)]
    dirty: bool,
}

impl Playlist {
    pub fn new(uuid: Uuid, name: String, description: String) -> Self {
        Playlist {
            uuid,
            name,
            description,
            contents: Vec::new(),
            dirty: true,
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
        let indices = self.contents.iter().enumerate().filter_map(|(idx, entry)| if entry.uuid() == &uuid { Some(idx) } else { None }).rev().collect::<Vec<_>>();
        indices.iter().for_each(|idx| {
            self.contents.remove(*idx);
        });
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
        self.contents
            .iter()
            .map(|entry| match entry {
                PlaylistEntry::Playlist(id) => library.playlists.get(id).map(|playlist| playlist.flat_len(library)).unwrap_or(0),
                PlaylistEntry::Song(_) => 1,
            }).sum()
    }
}

#[derive(Serialize, Deserialize, Copy, Clone)]
#[serde(tag = "type", content = "uuid", rename_all = "lowercase")]
pub enum PlaylistEntry {
    Song(Uuid),
    Playlist(Uuid),
}

impl PlaylistEntry {
    pub fn uuid(&self) -> &Uuid {
        match self {
            PlaylistEntry::Song(id) => id,
            PlaylistEntry::Playlist(id) => id,
        }
    }
}

pub struct MediaLibrary {
    songs: HashMap<Uuid, AudioSource>,
    playlists: HashMap<Uuid, Playlist>,
    dirty: bool,
    config: Config,
}

impl MediaLibrary {
    fn new(config: Config) -> Self {
        MediaLibrary {
            songs: HashMap::new(),
            playlists: HashMap::new(),
            dirty: false,
            config
        }
    }

    pub fn save(&mut self) -> Result<(), Report<LoadSaveError>> {
        let data_dir = Path::new(&self.config.data_directory);
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
            let mut songs = self.songs.values().collect::<Vec<_>>();
            songs.sort_unstable_by_key(|s| s.uuid);
            // This should never fail
            let data = serde_json::to_string(&songs).expect("Failed to serialize library");
            file.write_all(data.as_bytes()).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to write library to file"))?;
            file.sync_all().map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to sync library file"))?;
            fs::rename(new_library_file, library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to overwrite old library file with new library file"))?;
            self.dirty = false;
        }

        for playlist in self.playlists.values_mut().filter(|p| p.dirty) {
            let playlist_file = playlist_dir.join(playlist.uuid.to_string()).with_added_extension("json");
            let temp_file = playlist_file.with_added_extension("new");
            let mut file = File::create(&temp_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to create playlist file"))?;
            let data = serde_json::to_string(&playlist).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to serialize playlist file"))?;
            file.write_all(data.as_bytes()).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to write playlist file to file"))?;
            file.sync_all().map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to sync playlist file"))?;
            fs::rename(temp_file, playlist_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to overwrite playlist file with new playlist file"))?;
            playlist.mark_clean();
        }

        Ok(())
    }

    pub fn export_backup(&self, path: &Path) -> Result<(), Report<LoadSaveError>> {
        write_zip_atomic(path, |archive| {
            let manifest = serde_json::to_vec(&BackupManifest { format_version: 1 })
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to serialize backup manifest"))?;
            archive.start_file("manifest.json", SimpleFileOptions::default())
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add backup manifest"))?;
            archive.write_all(&manifest)
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to write backup manifest"))?;

            let mut songs = self.songs.values().collect::<Vec<_>>();
            songs.sort_unstable_by_key(|song| song.uuid);
            let mut exported_songs = Vec::new();
            for song in songs {
                let archive_path = backup_song_path(song.uuid, &song.path);
                let mut source = match File::open(&song.path) {
                    Ok(source) => source,
                    Err(error) => {
                        eprintln!("Skipping song {} during backup: {error}", song.uuid);
                        continue;
                    }
                };

                archive.start_file(&archive_path, SimpleFileOptions::default().compression_method(CompressionMethod::Stored))
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add audio file to backup"))?;
                std::io::copy(&mut source, archive)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to copy audio file to backup"))?;

                let mut exported_song = song.clone();
                exported_song.path = PathBuf::from(archive_path);
                exported_songs.push(exported_song);
            }

            archive.start_file("library.json", SimpleFileOptions::default())
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add library data to backup"))?;
            serde_json::to_writer(&mut *archive, &exported_songs)
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to write library data to backup"))?;

            let mut playlists = self.playlists.values().collect::<Vec<_>>();
            playlists.sort_unstable_by_key(|playlist| playlist.uuid);
            for playlist in playlists {
                archive.start_file(format!("playlists/{}.json", playlist.uuid), SimpleFileOptions::default())
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add playlist to backup"))?;
                serde_json::to_writer(&mut *archive, playlist)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to write playlist to backup"))?;
            }

            Ok(())
        })
    }

    pub fn export_m3u(&self, path: &Path) -> Result<(), Report<LoadSaveError>> {
        write_zip_atomic(path, |archive| {
            let mut songs = self.songs.values().collect::<Vec<_>>();
            songs.sort_by_key(|song| (
                song.artist.as_deref().unwrap_or("").to_lowercase(),
                song.title.to_lowercase(),
                song.uuid,
            ));

            let mut used_song_names = HashSet::new();
            let mut exported_song_paths = HashMap::new();
            for song in &songs {
                let artist = song.artist.as_deref().unwrap_or("").trim();
                let title = song.title.trim();
                let base_name = if artist.is_empty() { title.to_string() } else { format!("{artist} - {title}") };
                let file_name = unique_filename(&base_name, &safe_extension(&song.path), &mut used_song_names);
                let mut source = match File::open(&song.path) {
                    Ok(source) => source,
                    Err(error) => {
                        eprintln!("Skipping song {} during M3U export: {error}", song.uuid);
                        continue;
                    }
                };

                archive.start_file(format!("songs/{file_name}"), SimpleFileOptions::default().compression_method(CompressionMethod::Stored))
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add audio file to M3U archive"))?;
                std::io::copy(&mut source, archive)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to copy audio file to M3U archive"))?;
                exported_song_paths.insert(song.uuid, format!("songs/{file_name}"));
            }

            let mut all_songs = String::from("#EXTM3U\n");
            for song in &songs {
                if let Some(relative_path) = exported_song_paths.get(&song.uuid) {
                    append_m3u_entry(&mut all_songs, song, relative_path);
                }
            }
            archive.start_file("All Songs.m3u", SimpleFileOptions::default())
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add All Songs playlist"))?;
            archive.write_all(all_songs.as_bytes())
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to write All Songs playlist"))?;

            let mut playlists = self.playlists.values().collect::<Vec<_>>();
            playlists.sort_by_key(|playlist| (playlist.name.to_lowercase(), playlist.uuid));
            let mut used_playlist_names = HashSet::from(["all songs.m3u".to_string()]);
            for playlist in playlists {
                let playlist_file_name = unique_filename(&playlist.name, "m3u", &mut used_playlist_names);
                let mut entries = String::from("#EXTM3U\n");
                let mut flattened_songs = Vec::new();
                self.flatten_playlist(playlist.uuid, &mut HashSet::new(), &mut flattened_songs);
                for uuid in flattened_songs {
                    if let (Some(song), Some(relative_path)) = (self.songs.get(&uuid), exported_song_paths.get(&uuid)) {
                        append_m3u_entry(&mut entries, song, relative_path);
                    }
                }

                archive.start_file(playlist_file_name, SimpleFileOptions::default())
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to add playlist M3U file"))?;
                archive.write_all(entries.as_bytes())
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to write playlist M3U file"))?;
            }

            Ok(())
        })
    }

    pub fn import_backup(&mut self, path: &Path) -> Result<ImportSummary, Report<LoadSaveError>> {
        let file = File::open(path).map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to open library backup"))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to read library backup"))?;
        let mut songs: Vec<AudioSource> = {
            let mut library_file = archive.by_name("library.json")
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Backup does not contain library.json"))?;
            serde_json::from_reader(&mut library_file)
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to parse backup library"))?
        };

        let song_directory = Path::new(&self.config.data_directory).join("songs");
        fs::create_dir_all(&song_directory)
            .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to create imported song directory"))?;
        let mut summary = ImportSummary::default();
        for song in &mut songs {
            if self.songs.contains_key(&song.uuid) {
                continue;
            }

            let archive_path = backup_song_path(song.uuid, &song.path);
            let destination = song_directory.join(backup_song_filename(song.uuid, &song.path));
            let temporary = destination.with_added_extension("tmp");
            let copy_result = (|| -> Result<(), Report<LoadSaveError>> {
                let mut source = archive.by_name(&archive_path)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Backup is missing an audio file"))?;
                let mut output = File::create(&temporary)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to create imported audio file"))?;
                std::io::copy(&mut source, &mut output)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to extract imported audio file"))?;
                output.sync_all()
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to sync imported audio file"))?;
                fs::rename(&temporary, &destination)
                    .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to install imported audio file"))?;
                Ok(())
            })();
            if let Err(error) = copy_result {
                let _ = fs::remove_file(&temporary);
                eprintln!("Skipping song {} during library import: {error:?}", song.uuid);
                continue;
            }

            song.path = destination;
            let imported_song = read_song_metadata(&song.path, song.uuid).unwrap_or_else(|| song.clone());
            self.songs.insert(song.uuid, imported_song);
            self.dirty = true;
            summary.songs += 1;
        }

        let mut playlist_candidates = Vec::new();
        let mut candidate_uuids = HashSet::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)
                .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to read playlist from backup"))?;
            let name = match entry.name() {
                Ok(name) => name.into_owned(),
                Err(error) => {
                    eprintln!("Skipping playlist with an invalid archive name: {error}");
                    continue;
                }
            };
            if !name.starts_with("playlists/") || !name.ends_with(".json") || entry.is_dir() {
                continue;
            }

            let mut data = Vec::new();
            if let Err(error) = entry.read_to_end(&mut data) {
                eprintln!("Skipping unreadable playlist in backup: {error}");
                continue;
            }
            let playlist: Playlist = match serde_json::from_slice(&data) {
                Ok(playlist) => playlist,
                Err(error) => {
                    eprintln!("Skipping invalid playlist in backup: {error}");
                    continue;
                }
            };
            if self.playlists.contains_key(&playlist.uuid) || !candidate_uuids.insert(playlist.uuid) {
                continue;
            }

            let playlist_uuid = playlist.uuid;
            let playlist_name = playlist.name.clone();
            let playlist_description = playlist.description.clone();
            let entries = playlist.contents.clone();
            self.playlists.insert(playlist_uuid, Playlist::new(playlist_uuid, playlist_name, playlist_description));
            playlist_candidates.push((playlist_uuid, entries));
            summary.playlists += 1;
        }

        for (playlist_uuid, entries) in playlist_candidates {
            for entry in entries {
                let exists = match entry {
                    PlaylistEntry::Song(uuid) => self.songs.contains_key(&uuid),
                    PlaylistEntry::Playlist(uuid) => self.playlists.contains_key(&uuid),
                };
                if exists {
                    // This also rejects nested playlist entries that would introduce a cycle.
                    let _ = self.add_to_playlist(entry, playlist_uuid);
                }
            }
        }

        self.save()?;
        Ok(summary)
    }

    fn flatten_playlist(&self, uuid: Uuid, visiting: &mut HashSet<Uuid>, songs: &mut Vec<Uuid>) {
        if !visiting.insert(uuid) {
            return;
        }
        if let Some(playlist) = self.playlists.get(&uuid) {
            for entry in &playlist.contents {
                match entry {
                    PlaylistEntry::Song(song_uuid) if self.songs.contains_key(song_uuid) => songs.push(*song_uuid),
                    PlaylistEntry::Playlist(playlist_uuid) => self.flatten_playlist(*playlist_uuid, visiting, songs),
                    _ => (),
                }
            }
        }
        visiting.remove(&uuid);
    }

    pub fn load(config: Config) -> Result<Self, Report<LoadSaveError>> {
        let mut library = Self::new(config);

        let data_dir = Path::new(&library.config.data_directory);
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
                return Err(LoadSaveError::from(e).into_report().attach("Failed to write library to file"));
            }
        }

        library.load_songs(library_file)?;
        library.load_playlists(playlist_dir)?;

        Ok(library)
    }

    fn load_songs(&mut self, library_file: impl AsRef<Path>) -> Result<(), Report<LoadSaveError>> {
        let library_file = File::open(library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to open library file"))?;
        let songs: Vec<AudioSource> = serde_json::from_reader(library_file).map_err(|e| LoadSaveError::from(e).into_report().attach("Failed to parse library"))?;
        for song in songs {
            let path = song.path.clone();
            // fallback to the metadata read from the library file if we cannot read the metadata from the file (e.g. song doesn't exist on disk or is not of a compatible file type with the metadata parsing library)
            let song = match read_song_metadata(&path, song.uuid) {
                Some(new_song) => {
                    if song != new_song {
                        self.dirty = true;
                    }
                    new_song
                }
                None => {
                    println!("Failed to read metadata for file {:?}: Using metadata stored in library", path);
                    song
                }
            };
            self.songs.insert(song.uuid, song);
        }
        Ok(())
    }

    fn load_playlists(&mut self, playlist_dir: impl AsRef<Path>) -> Result<(), Report<LoadSaveError>> {
        let playlists: Vec<_> = playlist_dir.as_ref().read_dir().map_err(|e| LoadSaveError::from(e).into_report().attach("Could not read playlist directory"))?.filter_map(|playlist_file| {
            let file = playlist_file.ok()?;
            if file.path().extension() == Some("json".as_ref()) {
                let file = File::open(file.path()).ok()?;
                let playlist: Option<Playlist> = serde_json::from_reader(file).ok();
                playlist
            } else {
                None
            }
        }).collect();

        playlists.into_iter().for_each(|playlist| {
            self.playlists.insert(playlist.uuid, playlist);
        });

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

    pub fn add_song(&mut self, path: PathBuf) -> Option<Uuid> {
        let canonical_path = path.canonicalize().ok()?;
        for song in self.songs.values() {
            if let Ok(song_path) = song.path.canonicalize() && song_path == canonical_path {
                return None;
            }
        }

        let uuid = loop {
            let uuid = Uuid::new_v4();
            if !self.songs.contains_key(&uuid) {
                break uuid;
            }
        };
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
                duration: None,
            }
        };

        self.songs.insert(uuid, song);
        self.dirty = true;

        Some(uuid)
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
        let uuid = loop {
            let uuid = Uuid::new_v4();
            if !self.playlists.contains_key(&uuid) {
                break uuid;
            }
        };
        let playlist = Playlist::new(uuid, name, description);
        self.playlists.insert(uuid, playlist);
        uuid
    }

    pub fn delete_playlist(&mut self, uuid: Uuid) -> bool {
        let playlist = self.playlists.remove(&uuid);
        if let Some(playlist) = playlist {
            let uuid = playlist.uuid;
            self.playlists.iter_mut().for_each(|(_, playlist)| {
                playlist.remove_all(uuid);
            });

            let playlist_file = Path::new(&self.config.data_directory).join("playlists").join(format!("{}.json", uuid));
            let result = fs::remove_file(playlist_file);
            if result.is_err() {
                println!("Failed to delete playlist with uuid, name: {}, {}", uuid, playlist.name);
            }
            self.save().unwrap_or_else(|_| {
                println!("Failed to save library after deleting playlist");
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
        Ok(())
    }

    pub fn remove_from_playlist(&mut self, idx: usize, playlist_uuid: Uuid) -> Result<PlaylistEntry, Report<LibraryError>> {
        if let Some(playlist) = self.playlists.get_mut(&playlist_uuid) {
            let entry = playlist.remove(idx);
            Ok(entry)
        } else {
            Err(Report::new(LibraryError::UnknownPlaylist(playlist_uuid)))
        }
    }

    pub fn get_songs(&self) -> Vec<AudioSource> {
        self.songs.values().cloned().collect()
    }

    pub fn get_song_uuids(&self) -> Vec<Uuid> {
        self.songs.keys().cloned().collect()
    }

    pub fn get_song(&self, uuid: &Uuid) -> Option<AudioSource> {
        self.songs.get(uuid).cloned()
    }

    pub fn get_playlists(&self) -> Vec<Uuid> {
        self.playlists.keys().cloned().collect()
    }

    pub fn get_playlist_info(&self, uuid: &Uuid) -> Option<PlaylistInfo> {
        let playlist = self.playlists.get(uuid)?;
        Some(PlaylistInfo {
            uuid: playlist.uuid,
            name: playlist.name.clone(),
            description: playlist.description.clone(),
        })
    }

    pub fn get_playlist_contents(&self, uuid: Uuid) -> Option<Vec<PlaylistEntry>> {
        self.playlists.get(&uuid).map(|playlist| playlist.contents.clone())
    }

    pub fn get_playlist_length(&self, uuid: Uuid) -> Option<usize> {
        self.playlists.get(&uuid).map(|playlist| playlist.flat_len(self))
    }

    pub fn refresh_song_metadata(&mut self, uuid: Uuid) -> Result<bool, Report<LibraryError>> {
        let song = self.songs.get(&uuid).ok_or_else(|| Report::new(LibraryError::UnknownSong(uuid)))?;
        let updated_song = read_song_metadata(&song.path, uuid);
        if let Some(updated_song) = updated_song && song != &updated_song {
            self.songs.insert(uuid, updated_song);
            self.dirty = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn write_zip_atomic(
    path: &Path,
    write_entries: impl FnOnce(&mut ZipWriter<File>) -> Result<(), Report<LoadSaveError>>,
) -> Result<(), Report<LoadSaveError>> {
    let temporary_path = path.with_added_extension("tmp");
    let result = (|| {
        let file = File::create(&temporary_path)
            .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to create temporary export archive"))?;
        let mut archive = ZipWriter::new(file);
        write_entries(&mut archive)?;
        let file = archive.finish()
            .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to finish export archive"))?;
        file.sync_all()
            .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to sync export archive"))?;
        fs::rename(&temporary_path, path)
            .map_err(|error| LoadSaveError::from(error).into_report().attach("Failed to install export archive"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    result
}

fn backup_song_path(uuid: Uuid, path: &Path) -> String {
    format!("songs/{}", backup_song_filename(uuid, path))
}

fn backup_song_filename(uuid: Uuid, path: &Path) -> String {
    format!("{uuid}.{}", safe_extension(path))
}

fn safe_extension(path: &Path) -> String {
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or("");
    let sanitized = extension.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if sanitized.is_empty() { "audio".to_string() } else { sanitized }
}

fn safe_filename_component(value: &str) -> String {
    let mut sanitized = value.chars().map(|character| {
        if character.is_control() || matches!(character, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' | '#') {
            '_'
        } else {
            character
        }
    }).collect::<String>();
    sanitized = sanitized.trim().trim_matches('.').to_string();
    if sanitized.is_empty() { "Untitled".to_string() } else { sanitized }
}

fn unique_filename(base_name: &str, extension: &str, used_names: &mut HashSet<String>) -> String {
    let base_name = safe_filename_component(base_name);
    let extension = safe_extension(Path::new(&format!("file.{extension}")));
    let mut suffix = 1;
    loop {
        let candidate = if suffix == 1 {
            format!("{base_name}.{extension}")
        } else {
            format!("{base_name} ({suffix}).{extension}")
        };
        if used_names.insert(candidate.to_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn append_m3u_entry(output: &mut String, song: &AudioSource, relative_path: &str) {
    let duration = song.duration.map(|duration| duration.as_secs().min(i64::MAX as u64) as i64).unwrap_or(-1);
    let artist = song.artist.as_deref().map(safe_m3u_text).unwrap_or_default();
    let title = safe_m3u_text(&song.title);
    let display_name = if artist.is_empty() { title } else { format!("{artist} - {title}") };
    output.push_str(&format!("#EXTINF:{duration},{display_name}\n{relative_path}\n"));
}

fn safe_m3u_text(value: &str) -> String {
    value.chars().map(|character| if character.is_control() { ' ' } else { character }).collect::<String>()
}

fn read_song_metadata(path: impl AsRef<Path>, uuid: Uuid) -> Option<AudioSource> {
    let file = File::open(&path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.as_ref().extension().and_then(|x| x.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).ok()?;
    let mut metadata = format.metadata();

    let mut title = String::new();
    let mut artist = None;
    let mut album = None;
    let mut genre = None;
    let mut album_artist = None;
    let mut track_number = None;
    let mut track_count = None;
    let mut year = None;

    while let Some(meta) = metadata.pop() {
        for tag in meta.media.tags {
            match tag.std {
                Some(StandardTag::TrackTitle(meta_title)) => title = meta_title.to_string(),
                Some(StandardTag::Artist(meta_artist)) => artist = Some(meta_artist.to_string()),
                Some(StandardTag::Album(meta_album)) => album = Some(meta_album.to_string()),
                Some(StandardTag::Genre(meta_genre)) => genre = Some(meta_genre.to_string()),
                Some(StandardTag::AlbumArtist(meta_album_artist)) => album_artist = Some(meta_album_artist.to_string()),
                Some(StandardTag::TrackNumber(meta_track_number)) => track_number = Some(meta_track_number as u32),
                Some(StandardTag::TrackTotal(meta_track_total)) => track_count = Some(meta_track_total as u32),
                Some(StandardTag::ReleaseYear(meta_year)) => year = Some(meta_year.to_string()),
                _ => (),
            }
        }
    }
    // pop never removes the last metadata element, so after popping there will be one metadata element left that needs to be read unless there was no metadata in the first place.
    if let Some(meta) = metadata.current() {
        for tag in &meta.media.tags {
            match &tag.std {
                Some(StandardTag::TrackTitle(meta_title)) => title = meta_title.to_string(),
                Some(StandardTag::Artist(meta_artist)) => artist = Some(meta_artist.to_string()),
                Some(StandardTag::Album(meta_album)) => album = Some(meta_album.to_string()),
                Some(StandardTag::Genre(meta_genre)) => genre = Some(meta_genre.to_string()),
                Some(StandardTag::AlbumArtist(meta_album_artist)) => album_artist = Some(meta_album_artist.to_string()),
                Some(StandardTag::TrackNumber(meta_track_number)) => track_number = Some(*meta_track_number as u32),
                Some(StandardTag::TrackTotal(meta_track_total)) => track_count = Some(*meta_track_total as u32),
                Some(StandardTag::ReleaseYear(meta_year)) => year = Some(meta_year.to_string()),
                _ => (),
            }
        }
    }

    let track = format.default_track(TrackType::Audio)?;
    let duration = if let Some(time_base) = track.time_base && let Some(duration) = track.duration {
        time_base.calc_duration(duration).map(|t| Duration::from_nanos_u128(t.as_nanos() as u128)).filter(|duration| !duration.is_zero())
    } else {
        None
    };

    if title.is_empty() {
        title = path.as_ref().file_name()?.to_string_lossy().to_string();
    }

    Some(AudioSource {
        uuid,
        path: path.as_ref().to_path_buf(),
        title,
        artist,
        album,
        genre,
        album_artist,
        track_number,
        track_count,
        year,
        duration,
    })
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub data_directory: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("media-player-{label}-{}", Uuid::new_v4()))
    }

    fn library_with_songs(data_directory: PathBuf) -> (MediaLibrary, Vec<Uuid>) {
        let source_directory = data_directory.join("source");
        fs::create_dir_all(&source_directory).unwrap();
        let mut library = MediaLibrary::new(Config { data_directory });
        let mut uuids = Vec::new();
        for (index, (artist, title)) in [("Artist", "Track One"), ("Artist", "Track Two")].into_iter().enumerate() {
            let path = source_directory.join(format!("track-{index}.mp3"));
            fs::write(&path, format!("fake audio data {index}")).unwrap();
            let uuid = Uuid::new_v4();
            library.songs.insert(uuid, AudioSource {
                uuid,
                path,
                title: title.to_string(),
                artist: Some(artist.to_string()),
                album: None,
                genre: None,
                album_artist: None,
                track_number: None,
                track_count: None,
                year: None,
                duration: Some(Duration::from_secs(125 + index as u64)),
            });
            uuids.push(uuid);
        }
        library.dirty = true;
        (library, uuids)
    }

    #[test]
    fn backup_round_trip_preserves_uuids_audio_and_nested_playlists() {
        let root = test_directory("backup-round-trip");
        let (mut original, song_uuids) = library_with_songs(root.join("original"));
        let nested_uuid = original.create_playlist("Nested set".to_string(), String::new());
        let playlist_uuid = original.create_playlist("Road mix".to_string(), String::new());
        assert!(original.add_to_playlist(PlaylistEntry::Song(song_uuids[0]), nested_uuid).is_ok());
        assert!(original.add_to_playlist(PlaylistEntry::Playlist(nested_uuid), playlist_uuid).is_ok());
        assert!(original.add_to_playlist(PlaylistEntry::Song(song_uuids[1]), playlist_uuid).is_ok());
        let backup_path = root.join("library.zip");
        original.export_backup(&backup_path).unwrap();

        let file = File::open(&backup_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let manifest: serde_json::Value = serde_json::from_reader(archive.by_name("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["formatVersion"], 1);
        assert!(archive.by_name(&format!("playlists/{playlist_uuid}.json")).is_ok());

        let mut imported = MediaLibrary::new(Config { data_directory: root.join("imported") });
        let summary = imported.import_backup(&backup_path).unwrap();
        assert_eq!(summary.songs, 2);
        assert_eq!(summary.playlists, 2);
        for (index, uuid) in song_uuids.iter().copied().enumerate() {
            let song = imported.get_song(&uuid).unwrap();
            assert_eq!(song.uuid, uuid);
            assert!(song.path.exists());
            assert_eq!(fs::read(song.path).unwrap(), format!("fake audio data {index}").into_bytes());
        }
        assert_eq!(imported.get_playlist_length(playlist_uuid), Some(2));
        assert_eq!(imported.get_playlist_contents(playlist_uuid).unwrap().len(), 2);
        assert_eq!(imported.get_playlist_info(&playlist_uuid).unwrap().name, "Road mix");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn m3u_export_copies_songs_and_flattens_nested_playlists() {
        let root = test_directory("m3u-export");
        let (mut library, song_uuids) = library_with_songs(root.join("library"));
        let nested_uuid = library.create_playlist("Nested Playlist".to_string(), String::new());
        let playlist_uuid = library.create_playlist("Road Mix".to_string(), String::new());
        let reserved_name_uuid = library.create_playlist("All Songs".to_string(), String::new());
        assert!(library.add_to_playlist(PlaylistEntry::Song(song_uuids[0]), nested_uuid).is_ok());
        assert!(library.add_to_playlist(PlaylistEntry::Playlist(nested_uuid), playlist_uuid).is_ok());
        assert!(library.add_to_playlist(PlaylistEntry::Song(song_uuids[1]), playlist_uuid).is_ok());
        assert!(library.add_to_playlist(PlaylistEntry::Song(song_uuids[0]), reserved_name_uuid).is_ok());
        let export_path = root.join("players.zip");
        library.export_m3u(&export_path).unwrap();

        let file = File::open(&export_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut all_songs = String::new();
        archive.by_name("All Songs.m3u").unwrap().read_to_string(&mut all_songs).unwrap();
        assert_eq!(all_songs.matches("#EXTINF:").count(), 2);
        assert!(all_songs.contains("songs/Artist - Track One.mp3"));

        let mut playlist = String::new();
        archive.by_name("Road Mix.m3u").unwrap().read_to_string(&mut playlist).unwrap();
        assert_eq!(playlist.matches("#EXTINF:").count(), 2);
        assert!(playlist.contains("songs/Artist - Track One.mp3"));
        assert!(playlist.contains("songs/Artist - Track Two.mp3"));
        assert!(archive.by_name("Nested Playlist.m3u").is_ok());
        assert!(archive.by_name("All Songs (2).m3u").is_ok());
        assert!(archive.by_name("songs/Artist - Track Two.mp3").is_ok());

        fs::remove_dir_all(root).unwrap();
    }
}
