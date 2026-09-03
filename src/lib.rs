use std::collections::HashMap;
use std::num::NonZeroU32;
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
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    #[serde(deserialize_with = "ok_or_default")]
    pub track_count: Option<NonZeroU32>,
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

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "uuid", rename_all = "lowercase")]
pub enum PlaylistEntry {
    Song(Uuid),
    Playlist(Uuid)
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
    // TODO take config as a parameter
    pub fn load() -> MediaLibrary {
        MediaLibrary {
            songs: HashMap::new(),
            playlists: HashMap::new()
        }
    }
}