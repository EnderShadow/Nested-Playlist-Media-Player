use crate::MEDIA_LIBRARY;

use iced::widget::{
    button, column, container, responsive, row, rule::horizontal as horizontal_rule, scrollable,
    slider, space, text,
};
use iced::{Alignment, Element, Length, Size};
use media_player::{AudioSource, PlaylistInfo};
use uuid::Uuid;

const TABLE_ROW_HEIGHT: f32 = 42.0;
const TABLE_HEADER_HEIGHT: f32 = 42.0;
const MIN_VISIBLE_TABLE_ROWS: usize = 1;

#[derive(Default)]
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
    song_progress: f64,
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

        self.songs.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| {
                    a.artist
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .cmp(&b.artist.as_deref().unwrap_or("").to_lowercase())
                })
        });

        self.playlists = library
            .get_playlists()
            .into_iter()
            .filter_map(|uuid| {
                let info = library.get_playlist_info(&uuid)?;
                let entry_count = library
                    .get_playlist_contents(uuid)
                    .map(|contents| contents.len())
                    .unwrap_or_default();

                Some(PlaylistRow { info, entry_count })
            })
            .collect();

        self.playlists
            .sort_by(|a, b| a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()));

        if let Some(selected_song) = self.selected_song
            && selected_song >= self.songs.len()
        {
            self.selected_song = None;
        }

        self.first_visible_song = clamp_first_visible_row(
            self.first_visible_song,
            self.songs.len(),
            MIN_VISIBLE_TABLE_ROWS,
        );

        if let Some(selected_playlist) = self.selected_playlist
            && selected_playlist >= self.playlists.len()
        {
            self.selected_playlist = None;
        }

        self.first_visible_playlist = clamp_first_visible_row(
            self.first_visible_playlist,
            self.playlists.len(),
            MIN_VISIBLE_TABLE_ROWS,
        );
    }

    pub fn view(&self) -> Element<'_, UIMessage> {
        let tabs = row![
            tab_button(
                "Songs",
                MediaPlayerTab::Songs,
                self.selected_tab == MediaPlayerTab::Songs,
            ),
            tab_button(
                "Playlists",
                MediaPlayerTab::Playlists,
                self.selected_tab == MediaPlayerTab::Playlists,
            ),
        ]
        .spacing(8);

        let table = match self.selected_tab {
            MediaPlayerTab::Songs => self.song_table(),
            MediaPlayerTab::Playlists => self.playlist_table(),
        };

        column![
            tabs,
            horizontal_rule(1),
            table,
            horizontal_rule(1),
            self.media_controls(),
        ]
        .padding(16)
        .spacing(12)
        .into()
    }

    pub fn update(&mut self, message: UIMessage) {
        match message {
            UIMessage::SelectTab(selected_tab) => {
                self.selected_tab = selected_tab;
            }
            UIMessage::SelectSong(selected_song) => {
                self.selected_song = selected_song;

                if let Some(index) = selected_song
                    && let Some(song) = self.songs.get(index)
                {
                    self.current_song = Some(song.uuid);
                    self.song_progress = 0.0;
                }
            }
            UIMessage::SelectPlaylist(selected_playlist) => {
                self.selected_playlist = selected_playlist;
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
                        self.current_song = self.songs.get(index).map(|song| song.uuid);
                    } else if let Some(first_song) = self.songs.first() {
                        self.current_song = Some(first_song.uuid);
                        self.selected_song = Some(0);
                    }
                }

                self.player_state = MediaPlayerState::Playing;
            }
            UIMessage::Pause => {
                self.player_state = MediaPlayerState::Paused;
            }
            UIMessage::NextSong => {
                self.select_relative_song(1);
            }
            UIMessage::PreviousSong => {
                self.select_relative_song(-1);
            }
            UIMessage::ProgressChanged(progress) => {
                self.song_progress = progress;
            }
        }
    }

    fn song_table(&self) -> Element<'_, UIMessage> {
        responsive(|size| self.song_table_for_size(size)).into()
    }

    fn song_table_for_size(&self, size: Size) -> Element<'_, UIMessage> {
        let visible_rows = visible_table_rows(size);
        let start =
            clamp_first_visible_row(self.first_visible_song, self.songs.len(), visible_rows);
        let end = visible_end(start, self.songs.len(), visible_rows);

        let header = row![
            table_cell("Title", 3),
            table_cell("Artist", 2),
            table_cell("Album", 2),
            table_cell("Duration", 1),
        ]
        .spacing(8)
        .padding(8)
        .height(Length::Fixed(TABLE_HEADER_HEIGHT));

        let top_spacer_height = start as f32 * TABLE_ROW_HEIGHT;
        let bottom_spacer_height = self.songs.len().saturating_sub(end) as f32 * TABLE_ROW_HEIGHT;

        let rows = self.songs[start..end]
            .iter()
            .enumerate()
            .fold(
                column![space().height(Length::Fixed(top_spacer_height))].spacing(0),
                |rows, (visible_index, song)| {
                    let index = start + visible_index;
                    let selected = self.selected_song == Some(index);
                    let title = if selected {
                        format!("▶ {}", song.title)
                    } else {
                        song.title.clone()
                    };

                    let row = row![
                        table_cell(title, 3),
                        table_cell(song.artist.as_deref().unwrap_or("Unknown artist"), 2),
                        table_cell(song.album.as_deref().unwrap_or("Unknown album"), 2),
                        table_cell(format_duration(song.duration), 1),
                    ]
                    .spacing(8)
                    .padding(8)
                    .height(Length::Fixed(TABLE_ROW_HEIGHT))
                    .align_y(Alignment::Center);

                    rows.push(
                        button(row)
                            .width(Length::Fill)
                            .height(Length::Fixed(TABLE_ROW_HEIGHT))
                            .on_press(UIMessage::SelectSong(Some(index))),
                    )
                },
            )
            .push(space().height(Length::Fixed(bottom_spacer_height)));

        column![
            header,
            horizontal_rule(1),
            scrollable(rows)
                .height(Length::Fill)
                .width(Length::Fill)
                .on_scroll({
                    let total_rows = self.songs.len();

                    move |viewport| {
                        UIMessage::ScrollSongs(first_visible_row_from_scroll_offset(
                            viewport.absolute_offset().y,
                            total_rows,
                            visible_rows,
                        ))
                    }
                })
        ]
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
    }

    fn playlist_table(&self) -> Element<'_, UIMessage> {
        responsive(|size| self.playlist_table_for_size(size)).into()
    }

    fn playlist_table_for_size(&self, size: Size) -> Element<'_, UIMessage> {
        let visible_rows = visible_table_rows(size);
        let start = clamp_first_visible_row(
            self.first_visible_playlist,
            self.playlists.len(),
            visible_rows,
        );
        let end = visible_end(start, self.playlists.len(), visible_rows);

        let header = row![
            table_cell("Name", 3),
            table_cell("Description", 4),
            table_cell("Entries", 1),
        ]
        .spacing(8)
        .padding(8)
        .height(Length::Fixed(TABLE_HEADER_HEIGHT));

        let top_spacer_height = start as f32 * TABLE_ROW_HEIGHT;
        let bottom_spacer_height =
            self.playlists.len().saturating_sub(end) as f32 * TABLE_ROW_HEIGHT;

        let rows = self.playlists[start..end]
            .iter()
            .enumerate()
            .fold(
                column![space().height(Length::Fixed(top_spacer_height))].spacing(0),
                |rows, (visible_index, playlist)| {
                    let index = start + visible_index;
                    let selected = self.selected_playlist == Some(index);
                    let name = if selected {
                        format!("▶ {}", playlist.info.name)
                    } else {
                        playlist.info.name.clone()
                    };

                    let row = row![
                        table_cell(name, 3),
                        table_cell(&playlist.info.description, 4),
                        table_cell(playlist.entry_count.to_string(), 1),
                    ]
                    .spacing(8)
                    .padding(8)
                    .height(Length::Fixed(TABLE_ROW_HEIGHT))
                    .align_y(Alignment::Center);

                    rows.push(
                        button(row)
                            .width(Length::Fill)
                            .height(Length::Fixed(TABLE_ROW_HEIGHT))
                            .on_press(UIMessage::SelectPlaylist(Some(index))),
                    )
                },
            )
            .push(space().height(Length::Fixed(bottom_spacer_height)));

        column![
            header,
            horizontal_rule(1),
            scrollable(rows)
                .height(Length::Fill)
                .width(Length::Fill)
                .on_scroll({
                    let total_rows = self.playlists.len();

                    move |viewport| {
                        UIMessage::ScrollPlaylists(first_visible_row_from_scroll_offset(
                            viewport.absolute_offset().y,
                            total_rows,
                            visible_rows,
                        ))
                    }
                })
        ]
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
    }

    fn media_controls(&self) -> Element<'_, UIMessage> {
        let current_song_title = self
            .current_song
            .and_then(|uuid| self.songs.iter().find(|song| song.uuid == uuid))
            .map(|song| song.title.as_str())
            .unwrap_or("No song selected");

        let playback_status = match self.player_state {
            MediaPlayerState::Stopped => "Stopped",
            MediaPlayerState::Paused => "Paused",
            MediaPlayerState::Playing => "Playing",
        };

        let play_pause_button = if self.player_state == MediaPlayerState::Playing {
            button("Pause").on_press(UIMessage::Pause)
        } else {
            button("Play").on_press(UIMessage::Play)
        };

        column![
            row![
                text(current_song_title).size(18),
                space().width(Length::Fill),
                text(playback_status),
            ]
            .align_y(Alignment::Center),
            slider(0.0..=1.0, self.song_progress, UIMessage::ProgressChanged),
            row![
                button("Previous").on_press(UIMessage::PreviousSong),
                play_pause_button,
                button("Next").on_press(UIMessage::NextSong),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8)
        .into()
    }

    fn select_relative_song(&mut self, offset: isize) {
        if self.songs.is_empty() {
            self.selected_song = None;
            self.current_song = None;
            self.player_state = MediaPlayerState::Stopped;
            self.song_progress = 0.0;
            return;
        }

        let current_index = self
            .current_song
            .and_then(|uuid| self.songs.iter().position(|song| song.uuid == uuid))
            .or(self.selected_song)
            .unwrap_or(0);

        let next_index = if offset.is_negative() {
            current_index
                .checked_sub(offset.unsigned_abs())
                .unwrap_or(self.songs.len() - 1)
        } else {
            (current_index + offset as usize) % self.songs.len()
        };

        self.selected_song = Some(next_index);
        self.current_song = Some(self.songs[next_index].uuid);
        self.first_visible_song = first_visible_row_for_selection(
            next_index,
            self.first_visible_song,
            self.songs.len(),
            MIN_VISIBLE_TABLE_ROWS,
        );
        self.song_progress = 0.0;
    }
}

fn visible_table_rows(size: Size) -> usize {
    let available_height = size.height - TABLE_HEADER_HEIGHT;

    ((available_height / TABLE_ROW_HEIGHT).ceil() as usize)
        .saturating_add(1)
        .max(MIN_VISIBLE_TABLE_ROWS)
}

fn max_first_visible_row(total_rows: usize, visible_rows: usize) -> usize {
    total_rows.saturating_sub(visible_rows)
}

fn clamp_first_visible_row(
    first_visible_row: usize,
    total_rows: usize,
    visible_rows: usize,
) -> usize {
    first_visible_row.min(max_first_visible_row(total_rows, visible_rows))
}

fn first_visible_row(scroll_position: f64, total_rows: usize, visible_rows: usize) -> usize {
    let max_first_visible = max_first_visible_row(total_rows, visible_rows);

    ((clamp_scroll_position(scroll_position) * max_first_visible as f64).round() as usize)
        .min(max_first_visible)
}

fn first_visible_row_from_scroll_offset(
    scroll_offset: f32,
    total_rows: usize,
    visible_rows: usize,
) -> usize {
    let first_visible_row = (scroll_offset / TABLE_ROW_HEIGHT).floor() as usize;

    clamp_first_visible_row(first_visible_row, total_rows, visible_rows)
}

fn visible_end(first_visible_row: usize, total_rows: usize, visible_rows: usize) -> usize {
    first_visible_row
        .saturating_add(visible_rows)
        .min(total_rows)
}

fn first_visible_row_for_selection(
    selected_index: usize,
    current_first_visible_row: usize,
    total_rows: usize,
    visible_rows: usize,
) -> usize {
    let current_end = visible_end(current_first_visible_row, total_rows, visible_rows);

    if selected_index < current_first_visible_row {
        selected_index
    } else if selected_index >= current_end {
        selected_index
            .saturating_add(1)
            .saturating_sub(visible_rows)
            .min(max_first_visible_row(total_rows, visible_rows))
    } else {
        current_first_visible_row
    }
}

fn scroll_step(total_rows: usize, visible_rows: usize) -> f64 {
    let max_first_visible = max_first_visible_row(total_rows, visible_rows);

    if max_first_visible == 0 {
        1.0
    } else {
        1.0 / max_first_visible as f64
    }
}

fn clamp_scroll_position(scroll_position: f64) -> f64 {
    scroll_position.clamp(0.0, 1.0)
}

fn scroll_position_for_first_visible_row(
    first_visible_row: usize,
    total_rows: usize,
    visible_rows: usize,
) -> f64 {
    let max_first_visible = max_first_visible_row(total_rows, visible_rows);

    if max_first_visible == 0 {
        0.0
    } else {
        first_visible_row.min(max_first_visible) as f64 / max_first_visible as f64
    }
}

fn scroll_position_for_selected_row(
    selected_index: usize,
    current_scroll_position: f64,
    total_rows: usize,
) -> f64 {
    let visible_rows = MIN_VISIBLE_TABLE_ROWS;
    let current_first_visible =
        first_visible_row(current_scroll_position, total_rows, visible_rows);

    if selected_index < current_first_visible {
        scroll_position_for_first_visible_row(selected_index, total_rows, visible_rows)
    } else {
        let max_first_visible = max_first_visible_row(total_rows, visible_rows);
        let first_visible = selected_index.min(max_first_visible);

        scroll_position_for_first_visible_row(first_visible, total_rows, visible_rows)
    }
}

fn tab_button(
    label: &'static str,
    tab: MediaPlayerTab,
    selected: bool,
) -> iced::widget::Button<'static, UIMessage> {
    let label = if selected {
        format!("[ {label} ]")
    } else {
        label.to_string()
    };

    button(text(label)).on_press(UIMessage::SelectTab(tab))
}

fn table_cell<'a>(content: impl Into<String>, width_portion: u16) -> Element<'a, UIMessage> {
    container(text(content.into()))
        .width(Length::FillPortion(width_portion))
        .into()
}

fn format_duration(duration: std::time::Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;

    format!("{minutes}:{seconds:02}")
}

pub fn run_gui() -> iced::Result {
    iced::application(UIState::new, UIState::update, UIState::view).run()
}
