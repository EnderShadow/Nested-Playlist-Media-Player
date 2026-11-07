use std::fs::File;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use rodio::MixerDeviceSink;
use uuid::Uuid;
use media_player::{AudioSource, PlaylistEntry, PlaylistInfo};
use crate::MEDIA_LIBRARY;

static AUDIO_SINK_HANDLE: LazyLock<MixerDeviceSink> = LazyLock::new(|| {
    rodio::DeviceSinkBuilder::open_default_sink().expect("Failed to open default sink")
});

static PLAYER: LazyLock<Mutex<Player>> = LazyLock::new(|| Mutex::new(Player {
    current_song: None,
    current_index: None,
    player_state: PlayerState::Stopped,
    queue: Vec::new(),
    queue_state: Vec::new(),
    player: rodio::Player::connect_new(AUDIO_SINK_HANDLE.mixer())
}));

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum PlayerState {
    Stopped,
    Playing,
    Paused
}

struct Player {
    current_song: Option<Uuid>,
    current_index: Option<usize>,
    player_state: PlayerState,
    queue: Vec<PlaylistEntry>,
    queue_state: Vec<usize>,
    player: rodio::Player
}

impl Player {
    fn stop(&mut self, preserve_queue: bool) {
        self.player.stop();
        self.player_state = PlayerState::Stopped;
        if !preserve_queue {
            self.current_song = None;
            self.current_index = None;
            self.queue.clear();
            self.queue_state.clear();
        }
    }

    fn enqueue_song(&mut self, audio: &AudioSource) {
        self.queue.push(PlaylistEntry::Song(audio.uuid));
    }

    fn enqueue_playlist(&mut self, playlist: &PlaylistInfo) {
        self.queue.push(PlaylistEntry::Playlist(playlist.uuid))
    }

    fn play(&mut self) {
        match self.player_state {
            PlayerState::Stopped => {
                if self.current_song.is_none() {
                    self.next();
                }
                if let Some(current_song) = &self.current_song {
                    let song_path = MEDIA_LIBRARY.get().unwrap().read().unwrap().get_song_info(current_song).map(|x| x.path).unwrap();
                    let file = File::open(song_path).unwrap();
                    self.player.append(rodio::Decoder::try_from(file).unwrap());
                    self.player_state = PlayerState::Playing;
                }
            },
            PlayerState::Paused => {
                self.player.play();
                self.player_state = PlayerState::Playing;
            }
            PlayerState::Playing => {}
        }
    }

    fn next(&mut self) {
        let index = self.current_index.map(|i| i + 1).unwrap_or(0);
        let song = self.walk_queue(index);
        if let Some((uuid, queue_state)) = song {
            self.current_song = Some(uuid);
            self.current_index = Some(index);
            self.queue_state = queue_state;
        } else {
            self.stop(true);
            self.current_song = None;
            self.current_index = None;
            self.queue_state.clear();
        }
    }

    fn prev(&mut self) {
        let index = self.current_index.map(|i| i.saturating_sub(1)) .unwrap_or(0);
        let song = self.walk_queue(index);
        if let Some((uuid, queue_state)) = song {
            self.current_song = Some(uuid);
            self.current_index = Some(index);
            self.queue_state = queue_state;
        }
    }

    fn walk_queue(&mut self, index: usize) -> Option<(Uuid, Vec<usize>)> {
        if self.queue.is_empty() {
            return None;
        }
        let mut queue_stack = vec![0usize];
        let mut current_index = 0usize;
        let mut iter_stack = vec![];
        let mut current_iter = self.queue.clone().into_iter();
        loop {
            let entry = current_iter.next();
            if let Some(entry) = entry {
                match entry {
                    PlaylistEntry::Song(uuid) => {
                        if current_index == index {
                            return Some((uuid, queue_stack));
                        } else {
                            current_index += 1;
                            let last = queue_stack.pop().unwrap();
                            queue_stack.push(last + 1);
                        }
                    }
                    PlaylistEntry::Playlist(uuid) => {
                        iter_stack.push(current_iter);
                        current_iter = MEDIA_LIBRARY.get().unwrap().read().unwrap().get_playlist_contents(uuid).unwrap().into_iter();
                        queue_stack.push(0);
                    }
                }
            } else if !iter_stack.is_empty() {
                current_iter = iter_stack.pop().unwrap();
                queue_stack.pop();
                let last = queue_stack.pop().unwrap();
                queue_stack.push(last + 1);
            } else {
                return None;
            }
        }
    }

    fn seek(&mut self, position: Duration) {
        self.player.try_seek(position).expect("Failed to seek");
    }

    fn pause(&mut self) {
        if self.player_state == PlayerState::Playing {
            self.player.pause();
            self.player_state = PlayerState::Paused;
        }
    }
}

pub fn play_song(audio: &AudioSource) {
    let mut player = PLAYER.lock().unwrap();
    player.stop(false);
    player.enqueue_song(audio);
    player.next();
    player.play();
}

pub fn enqueue_song(audio: &AudioSource) {
    let mut player = PLAYER.lock().unwrap();
    player.enqueue_song(audio);
}

pub fn play_playlist(playlist: &PlaylistInfo) {
    let mut player = PLAYER.lock().unwrap();
    player.stop(false);
    player.enqueue_playlist(playlist);
    player.next();
    player.play();
}

pub fn enqueue_playlist(playlist: &PlaylistInfo) {
    let mut player = PLAYER.lock().unwrap();
    player.enqueue_playlist(playlist);
}

pub fn stop_player() {
    let mut player = PLAYER.lock().unwrap();
    player.stop(false);
}

pub fn pause_player() {
    let mut player = PLAYER.lock().unwrap();
    player.pause();
}

pub fn resume_player() {
    let mut player = PLAYER.lock().unwrap();
    player.play();
}

pub fn next_song() {
    let mut player = PLAYER.lock().unwrap();
    let paused = player.player_state == PlayerState::Paused;
    player.stop(true);
    player.next();
    player.play();
    if paused {
        player.pause();
    }
}

pub fn previous_song() {
    let mut player = PLAYER.lock().unwrap();
    let paused = player.player_state == PlayerState::Paused;
    player.stop(true);
    player.prev();
    player.play();
    if paused {
        player.pause();
    }
}

pub fn current_song() -> Option<Uuid> {
    let player = PLAYER.lock().unwrap();
    player.current_song
}

pub fn player_state() -> PlayerState {
    let player = PLAYER.lock().unwrap();
    player.player_state
}

pub fn song_progress() -> Duration {
    let player = PLAYER.lock().unwrap();
    player.player.get_pos()
}

pub fn seek(position: f64) {
    let mut player = PLAYER.lock().unwrap();
    if let Some(current_song) = player.current_song {
        let duration = MEDIA_LIBRARY.get().unwrap().read().unwrap().get_song_info(&current_song).unwrap().duration;
        player.seek(duration.mul_f64(position));
    }
}

pub fn set_volume(volume: f32) {
    let player = PLAYER.lock().unwrap();
    player.player.set_volume(volume);
}

pub fn get_queue() -> Vec<PlaylistEntry> {
    let player = PLAYER.lock().unwrap();
    player.queue.clone()
}