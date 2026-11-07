use std::time::Duration;
use crate::{MEDIA_LIBRARY};

use iced::widget::{button, column, container, responsive, row, rule::horizontal as horizontal_rule, scrollable, slider, space, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Size, Subscription, Theme};
use media_player::{AudioSource, PlaylistInfo};
use uuid::Uuid;
use crate::player::{current_song, PlayerState, player_state, play_song, resume_player, pause_player, song_progress, seek, set_volume, previous_song, next_song, enqueue_song};

const TABLE_ROW_HEIGHT: f32 = 42.0;
const TABLE_HEADER_HEIGHT: f32 = 42.0;
const MIN_VISIBLE_TABLE_ROWS: usize = 1;
const SONG_PROGRESS_DIVISIONS: f64 = 10000.0;

struct UIState {
    selected_tab: MediaPlayerTab,
    songs: Vec<AudioSource>,
    selected_song: Option<usize>,
    first_visible_song: usize,
    playlists: Vec<PlaylistRow>,
    selected_playlist: Option<usize>,
    first_visible_playlist: usize,
    player_state: MediaPlayerState,
    current_song: Option<Uuid>,
    current_song_duration: Duration,
    song_progress: f64,
    volume: f32,
}

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq)]
pub enum MediaPlayerTab {
    #[default]
    Songs,
    Playlists,
}

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq)]
pub enum MediaPlayerState {
    #[default]
    Stopped,
    Paused,
    Playing,
}

impl From<PlayerState> for MediaPlayerState {
    fn from(value: PlayerState) -> Self {
        match value {
            PlayerState::Stopped => MediaPlayerState::Stopped,
            PlayerState::Paused => MediaPlayerState::Paused,
            PlayerState::Playing => MediaPlayerState::Playing,
        }
    }
}

#[derive(Debug, Clone)]
struct PlaylistRow {
    info: PlaylistInfo,
    entry_count: usize,
}

#[derive(Debug, Clone)]
pub enum UIMessage {
    SelectTab(MediaPlayerTab),
    SelectSong(Option<usize>),
    SelectPlaylist(Option<usize>),
    ScrollSongs(usize),
    ScrollPlaylists(usize),
    Play,
    Pause,
    NextSong,
    PreviousSong,
    ProgressChanged(f64),
    VolumeChanged(f32),
    Tick,
}

impl Default for UIState {
    fn default() -> Self {
        Self {
            selected_tab: Default::default(),
            songs: Default::default(),
            selected_song: Default::default(),
            first_visible_song: Default::default(),
            playlists: Default::default(),
            selected_playlist: Default::default(),
            first_visible_playlist: Default::default(),
            player_state: Default::default(),
            current_song: Default::default(),
            current_song_duration: Duration::MAX,
            song_progress: Default::default(),
            volume: 100.0,
        }
    }
}

impl UIState {
    fn new() -> Self {
        let mut state = Self::default();
        state.reload_library_data();
        state
    }

    fn reload_library_data(&mut self) {
        let Some(library_lock) = MEDIA_LIBRARY.get() else {
            return;
        };

        let Ok(library) = library_lock.read() else {
            return;
        };

        self.songs = library.get_songs();

        self.songs.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.artist.as_deref().unwrap_or("").to_lowercase().cmp(&b.artist.as_deref().unwrap_or("").to_lowercase())));

        self.playlists = library.get_playlists().into_iter().filter_map(|uuid| {
                let info = library.get_playlist_info(&uuid)?;
                let entry_count = library.get_playlist_contents(uuid).map(|contents| contents.len()).unwrap_or_default();

                Some(PlaylistRow { info, entry_count })
            }).collect();

        self.playlists.sort_by_key(|playlist| playlist.info.name.to_lowercase());

        if let Some(selected_song) = self.selected_song && selected_song >= self.songs.len() {
            self.selected_song = None;
        }

        self.first_visible_song = clamp_first_visible_row(self.first_visible_song, self.songs.len(), MIN_VISIBLE_TABLE_ROWS);

        if let Some(selected_playlist) = self.selected_playlist && selected_playlist >= self.playlists.len() {
            self.selected_playlist = None;
        }

        self.first_visible_playlist = clamp_first_visible_row(self.first_visible_playlist, self.playlists.len(), MIN_VISIBLE_TABLE_ROWS);
    }

    fn subscription(&self) -> Subscription<UIMessage> {
        iced::time::every(Duration::from_millis(100)).map(|_| UIMessage::Tick)
    }

    pub fn view(&self) -> Element<'_, UIMessage> {
        let tabs = row![
            tab_button("Songs", MediaPlayerTab::Songs, self.selected_tab == MediaPlayerTab::Songs,),
            tab_button("Playlists", MediaPlayerTab::Playlists, self.selected_tab == MediaPlayerTab::Playlists,),
        ].spacing(8);

        let table = match self.selected_tab {
            MediaPlayerTab::Songs => self.song_table(),
            MediaPlayerTab::Playlists => self.playlist_table(),
        };

        column![tabs, horizontal_rule(1), table, horizontal_rule(1), self.media_controls(),].padding(16).spacing(12).into()
    }

    pub fn update(&mut self, message: UIMessage) {
        match message {
            UIMessage::SelectTab(selected_tab) => {
                self.selected_tab = selected_tab;
            }
            UIMessage::SelectSong(selected_song) => {
                if self.selected_song != selected_song {
                    self.selected_song = selected_song;
                } else if let Some(index) = selected_song && let Some(song) = self.songs.get(index) {
                    if self.player_state == MediaPlayerState::Playing {
                        enqueue_song(song);
                    } else {
                        play_song(song);
                    }
                }
            }
            UIMessage::SelectPlaylist(selected_playlist) => {
                if self.selected_playlist != selected_playlist {
                    self.selected_playlist = selected_playlist;
                } else {
                    // TODO: Open playlist
                }
            }
            UIMessage::ScrollSongs(first_visible_song) => {
                self.first_visible_song = first_visible_song;
            }
            UIMessage::ScrollPlaylists(first_visible_playlist) => {
                self.first_visible_playlist = first_visible_playlist;
            }
            UIMessage::Play => {
                if self.current_song.is_none() {
                    if let Some(index) = self.selected_song {
                        let song = self.songs.get(index).unwrap();
                        play_song(song);
                    } else if let Some(first_song) = self.songs.first() {
                        play_song(first_song);
                    }
                } else {
                    resume_player();
                }
            }
            UIMessage::Pause => {
                pause_player();
            }
            UIMessage::NextSong => {
                next_song()
            }
            UIMessage::PreviousSong => {
                if self.current_song_duration.mul_f64(self.song_progress / SONG_PROGRESS_DIVISIONS) > Duration::from_secs(5) {
                    seek(0.0);
                } else {
                    previous_song();
                }
            }
            UIMessage::ProgressChanged(progress) => {
                seek(progress / SONG_PROGRESS_DIVISIONS);
            }
            UIMessage::VolumeChanged(volume) => {
                self.volume = volume;
                set_volume(volume / 100.0);
            }
            UIMessage::Tick => {
                self.player_state = MediaPlayerState::from(player_state());
                self.current_song = current_song();
                self.current_song_duration = self.current_song.map(|uuid| MEDIA_LIBRARY.get().unwrap().read().unwrap().get_song_info(&uuid).map(|audio| audio.duration).unwrap_or(Duration::MAX)).unwrap_or(Duration::MAX);
                self.song_progress = SONG_PROGRESS_DIVISIONS * song_progress().as_secs_f64() / self.current_song_duration.as_secs_f64();
            }
        }
    }

    fn song_table(&self) -> Element<'_, UIMessage> {
        responsive(|size| self.song_table_for_size(size)).into()
    }

    fn song_table_for_size(&self, size: Size) -> Element<'_, UIMessage> {
        let visible_rows = visible_table_rows(size);
        let start = clamp_first_visible_row(self.first_visible_song, self.songs.len(), visible_rows);
        let end = visible_end(start, self.songs.len(), visible_rows);

        let header = row![table_cell("Title", 3), table_cell("Artist", 2), table_cell("Album", 2), table_cell("Duration", 1),].spacing(8).padding(8).height(Length::Fixed(TABLE_HEADER_HEIGHT));

        let top_spacer_height = start as f32 * TABLE_ROW_HEIGHT;
        let bottom_spacer_height = self.songs.len().saturating_sub(end) as f32 * TABLE_ROW_HEIGHT;

        let rows = self.songs[start..end].iter().enumerate()
            .fold(column![space().height(Length::Fixed(top_spacer_height))].spacing(0), |rows, (visible_index, song)| {
                let index = start + visible_index;
                let selected = self.selected_song == Some(index);

                let row = row![
                    table_cell(song.title.clone(), 3),
                    table_cell(song.artist.as_deref().unwrap_or("Unknown artist"), 2),
                    table_cell(song.album.as_deref().unwrap_or("Unknown album"), 2),
                    table_cell(format_duration(song.duration), 1),
                ].spacing(8).padding(8).height(Length::Fixed(TABLE_ROW_HEIGHT)).align_y(Alignment::Center);

                let row_button = button(row).width(Length::Fill).height(Length::Fixed(TABLE_ROW_HEIGHT)).on_press(UIMessage::SelectSong(Some(index)));

                let button = if selected {
                    row_button.style(selected_table_row_style)
                } else {
                    row_button
                };

                rows.push(button)
            }).push(space().height(Length::Fixed(bottom_spacer_height)));

        column![
            header,
            horizontal_rule(1),
            scrollable(rows).height(Length::Fill).width(Length::Fill).on_scroll({
                let total_rows = self.songs.len();

                move |viewport| UIMessage::ScrollSongs(first_visible_row_from_scroll_offset(viewport.absolute_offset().y, total_rows, visible_rows))
            })
        ].height(Length::Fill).width(Length::Fill).into()
    }

    fn playlist_table(&self) -> Element<'_, UIMessage> {
        responsive(|size| self.playlist_table_for_size(size)).into()
    }

    fn playlist_table_for_size(&self, size: Size) -> Element<'_, UIMessage> {
        let visible_rows = visible_table_rows(size);
        let start = clamp_first_visible_row(self.first_visible_playlist, self.playlists.len(), visible_rows);
        let end = visible_end(start, self.playlists.len(), visible_rows);

        let header = row![table_cell("Name", 3), table_cell("Description", 4), table_cell("Entries", 1),].spacing(8).padding(8).height(Length::Fixed(TABLE_HEADER_HEIGHT));

        let top_spacer_height = start as f32 * TABLE_ROW_HEIGHT;
        let bottom_spacer_height = self.playlists.len().saturating_sub(end) as f32 * TABLE_ROW_HEIGHT;

        let rows = self.playlists[start..end].iter().enumerate()
            .fold(column![space().height(Length::Fixed(top_spacer_height))].spacing(0), |rows, (visible_index, playlist)| {
                let index = start + visible_index;
                let selected = self.selected_playlist == Some(index);
                let name = playlist.info.name.clone();

                let row = row![table_cell(name, 3), table_cell(&playlist.info.description, 4), table_cell(playlist.entry_count.to_string(), 1),]
                    .spacing(8).padding(8).height(Length::Fixed(TABLE_ROW_HEIGHT)).align_y(Alignment::Center);

                let row_button = button(row).width(Length::Fill).height(Length::Fixed(TABLE_ROW_HEIGHT)).on_press(UIMessage::SelectPlaylist(Some(index)));

                let row_button = if selected {
                    row_button.style(selected_table_row_style)
                } else {
                    row_button
                };

                rows.push(row_button)
            }).push(space().height(Length::Fixed(bottom_spacer_height)));

        column![
            header,
            horizontal_rule(1),
            scrollable(rows).height(Length::Fill).width(Length::Fill).on_scroll({
                let total_rows = self.playlists.len();

                move |viewport| UIMessage::ScrollPlaylists(first_visible_row_from_scroll_offset(viewport.absolute_offset().y, total_rows, visible_rows))
            })
        ].height(Length::Fill).width(Length::Fill).into()
    }

    fn media_controls(&self) -> Element<'_, UIMessage> {
        let current_song_title = self.current_song.and_then(|uuid| self.songs.iter().find(|song| song.uuid == uuid)).map(|song| song.title.as_str()).unwrap_or("No song selected");

        let progress_text = if self.player_state != MediaPlayerState::Stopped && self.current_song_duration != Duration::MAX {
            format!("{} / {}", format_duration(self.current_song_duration.mul_f64(self.song_progress / SONG_PROGRESS_DIVISIONS)), format_duration(self.current_song_duration))
        } else {
            "-:-- / -:--".to_string()
        };

        let play_pause_button = if self.player_state == MediaPlayerState::Playing { button("Pause").on_press(UIMessage::Pause) } else { button("Play").on_press(UIMessage::Play) };

        column![
            row![text(current_song_title).size(18), space().width(Length::Fill), text(progress_text),].align_y(Alignment::Center),
            slider(0.0..=SONG_PROGRESS_DIVISIONS, self.song_progress, UIMessage::ProgressChanged),
            row![button("Previous").on_press(UIMessage::PreviousSong), play_pause_button, button("Next").on_press(UIMessage::NextSong), space().width(Length::FillPortion(8)), slider(0.0..=100.0, self.volume, UIMessage::VolumeChanged).width(Length::FillPortion(2))].spacing(8).align_y(Alignment::Center),
        ].spacing(8).into()
    }
}

fn visible_table_rows(size: Size) -> usize {
    let available_height = size.height - TABLE_HEADER_HEIGHT;

    ((available_height / TABLE_ROW_HEIGHT).ceil() as usize).saturating_add(1).max(MIN_VISIBLE_TABLE_ROWS)
}

fn max_first_visible_row(total_rows: usize, visible_rows: usize) -> usize {
    total_rows.saturating_sub(visible_rows)
}

fn clamp_first_visible_row(first_visible_row: usize, total_rows: usize, visible_rows: usize) -> usize {
    first_visible_row.min(max_first_visible_row(total_rows, visible_rows))
}

fn first_visible_row_from_scroll_offset(scroll_offset: f32, total_rows: usize, visible_rows: usize) -> usize {
    let first_visible_row = (scroll_offset / TABLE_ROW_HEIGHT).floor() as usize;

    clamp_first_visible_row(first_visible_row, total_rows, visible_rows)
}

fn visible_end(first_visible_row: usize, total_rows: usize, visible_rows: usize) -> usize {
    first_visible_row.saturating_add(visible_rows).min(total_rows)
}

fn selected_table_row_style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
    let background_color = match status {
        iced::widget::button::Status::Hovered => Color::from_rgb(0.18, 0.42, 0.75),
        iced::widget::button::Status::Pressed => Color::from_rgb(0.12, 0.32, 0.62),
        _ => Color::from_rgb(0.14, 0.36, 0.68),
    };

    iced::widget::button::Style {
        background: Some(Background::Color(background_color)),
        text_color: Color::WHITE,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..iced::widget::button::Style::default()
    }
}

fn tab_button(label: &'static str, tab: MediaPlayerTab, selected: bool) -> iced::widget::Button<'static, UIMessage> {
    let label = if selected { format!("[ {label} ]") } else { label.to_string() };

    button(text(label)).on_press(UIMessage::SelectTab(tab))
}

fn table_cell<'a>(content: impl Into<String>, width_portion: u16) -> Element<'a, UIMessage> {
    container(text(content.into())).width(Length::FillPortion(width_portion)).into()
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;

    format!("{minutes}:{seconds:02}")
}

pub fn run_gui() -> iced::Result {
    iced::application(UIState::new, UIState::update, UIState::view).subscription(UIState::subscription).run()
}
