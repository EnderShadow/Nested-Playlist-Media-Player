use std::fs::File;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use rand::prelude::{IndexedRandom, IteratorRandom};
use rodio::MixerDeviceSink;
use uuid::Uuid;
use media_player::{AudioSource, PlaylistEntry, PlaylistInfo};
use crate::MEDIA_LIBRARY;

static AUDIO_SINK_HANDLE: LazyLock<MixerDeviceSink> = LazyLock::new(|| {
    rodio::DeviceSinkBuilder::open_default_sink().expect("Failed to open default sink")
});

static PLAYER: LazyLock<Mutex<Player>> = LazyLock::new(|| {
    let player = Player {
        current_song: None,
        current_index: None,
        player_state: PlayerState::Stopped,
        generation: 0,
        queue: Vec::new(),
        queue_state: Vec::new(),
        player: rodio::Player::connect_new(AUDIO_SINK_HANDLE.mixer()),
        shuffle_enabled: false,
        loop_mode: LoopMode::None
    };
    player.player.set_volume(0.5);
    Mutex::new(player)
});

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum PlayerState {
    Stopped,
    Playing,
    Paused
}

#[derive(Default, Copy, Clone, Eq, PartialEq)]
pub enum LoopMode {
    #[default]
    None,
    Single,
    Queue
}

struct Player {
    current_song: Option<Uuid>,
    current_index: Option<usize>,
    player_state: PlayerState,
    generation: u64,
    queue: Vec<PlaylistEntry>,
    queue_state: Vec<usize>,
    player: rodio::Player,
    shuffle_enabled: bool,
    loop_mode: LoopMode
}

impl Player {
    fn stop(&mut self, preserve_queue: bool) {
        self.generation = self.generation.wrapping_add(1);
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
                    let song_path = MEDIA_LIBRARY.get().unwrap().read().unwrap().get_song(current_song).map(|x| x.path);
                    if let Some(song_path) = song_path {
                        let file = File::open(song_path);
                        if let Ok(file) = file {
                            let decoder = rodio::Decoder::try_from(file);
                            if let Ok(decoder) = decoder {
                                self.player.append(decoder);
                                let generation = self.generation;
                                self.player.append(rodio::source::EmptyCallback::new(Box::new(move || {
                                    std::thread::spawn(move || song_ended(generation));
                                })));
                                self.player_state = PlayerState::Playing;
                            }
                        }
                    }
                }
            },
            PlayerState::Paused => {
                self.player.play();
                self.player_state = PlayerState::Playing;
            }
            PlayerState::Playing => {}
        }
    }

    fn next(&mut self) -> bool {
        if self.shuffle_enabled {
            let media_library = MEDIA_LIBRARY.get().unwrap().read().unwrap();

            let queue_len = self.queue.iter().map(|playlist_entry| {
                if let PlaylistEntry::Playlist(uuid) = playlist_entry {
                    media_library.get_playlist_length(*uuid).unwrap_or(0)
                } else {
                    1
                }
            }).sum();

            // queue's of length 1 are intentionally discarded when shuffling since shuffling the entire library always gives queues of length 1 and repeating the same song is achievable with the loop mode.
            if queue_len > 1 {
                // release the lock on MEDIA_LIBRARY since walk_queue will need to acquire it.
                drop(media_library);
                let index = (0..queue_len).choose(&mut rand::rng()).unwrap();
                let (uuid, queue_state) = self.walk_queue(index).expect("Queue changed while we had an exclusive lock on it. This should never happen.");
                self.current_song = Some(uuid);
                self.current_index = Some(index);
                self.queue_state = queue_state;
                true
            } else if let Some(uuid) = media_library.get_song_uuids().choose(&mut rand::rng()).cloned() {
                self.current_song = Some(uuid);
                self.current_index = Some(0);
                self.queue_state.clear();
                self.queue_state.push(0);
                self.queue.clear();
                self.queue.push(PlaylistEntry::Song(uuid));
                true
            } else {
                self.stop(false);
                false
            }
        } else {
            let index = self.current_index.map(|i| i + 1).unwrap_or(0);
            let song = self.walk_queue(index);
            if let Some((uuid, queue_state)) = song {
                self.current_song = Some(uuid);
                self.current_index = Some(index);
                self.queue_state = queue_state;
                true
            } else {
                self.stop(true);
                self.current_song = None;
                self.current_index = None;
                self.queue_state.clear();
                false
            }
        }
    }

    fn prev(&mut self) -> bool {
        let index = self.current_index.map(|i| i.saturating_sub(1)) .unwrap_or(0);
        let song = self.walk_queue(index);
        if let Some((uuid, queue_state)) = song {
            self.current_song = Some(uuid);
            self.current_index = Some(index);
            self.queue_state = queue_state;
            true
        } else {
            false
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
                        let playlist_contents = MEDIA_LIBRARY.get().unwrap().read().unwrap().get_playlist_contents(uuid);
                        if let Some(playlist_contents) = playlist_contents {
                            iter_stack.push(current_iter);
                            current_iter = playlist_contents.into_iter();
                            queue_stack.push(0);
                        } else {
                            current_index += 1;
                            let last = queue_stack.pop().unwrap();
                            queue_stack.push(last + 1);
                        }
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
        let result = self.player.try_seek(position);
        if result.is_err() && let Some(uuid) = self.current_song{
            println!("Failed to seek to position {:.2} seconds in song with uuid: {}", position.as_secs_f32(), uuid);
        }
    }

    fn pause(&mut self) {
        if self.player_state == PlayerState::Playing {
            self.player.pause();
            self.player_state = PlayerState::Paused;
        }
    }
}

fn song_ended(generation: u64) {
    let mut player = PLAYER.lock().unwrap();
    if player.generation != generation || player.player_state != PlayerState::Playing {
        return;
    }
    player.stop(true);
    let should_play = match player.loop_mode {
        LoopMode::None => {
            player.next()
        }
        LoopMode::Single => {
            true
        }
        LoopMode::Queue => {
            player.next();
            true
        }
    };
    if should_play {
        player.play();
    }
}

pub fn play_song(audio: &AudioSource) {
    let mut player = PLAYER.lock().unwrap();
    player.stop(false);
    player.enqueue_song(audio);
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
    if player.next() {
        player.play();
    }
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

pub fn player_state() -> (PlayerState, bool, LoopMode) {
    let player = PLAYER.lock().unwrap();
    (player.player_state, player.shuffle_enabled, player.loop_mode)
}

pub fn song_progress() -> Duration {
    let player = PLAYER.lock().unwrap();
    player.player.get_pos()
}

pub fn seek(position: f64) {
    let mut player = PLAYER.lock().unwrap();
    if let Some(current_song) = player.current_song {
        let duration = MEDIA_LIBRARY.get().unwrap().read().unwrap().get_song(&current_song).unwrap().duration;
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

pub fn toggle_shuffle() {
    let mut player = PLAYER.lock().unwrap();
    player.shuffle_enabled = !player.shuffle_enabled;
}

pub fn cycle_loop_mode() {
    let mut player = PLAYER.lock().unwrap();
    match player.loop_mode {
        LoopMode::None => player.loop_mode = LoopMode::Single,
        LoopMode::Single => player.loop_mode = LoopMode::Queue,
        LoopMode::Queue => player.loop_mode = LoopMode::None,
    }
}