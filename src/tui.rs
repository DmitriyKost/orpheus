use std::collections::HashMap;
use std::{
    fs,
    io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use serde::{Deserialize, Serialize};

use crate::{config::Config, library::Track, playlist::{PlaylistStore, PlaylistSummary}, process};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Pane {
    Library,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum LeftView {
    Library,
    Playlists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Search,
    Command,
}

#[derive(Debug, Clone, Copy)]
struct Theme {
    pane_active: Color,
    pane_inactive: Color,
    pane_search: Color,
    list_highlight: Color,
    now_border: Color,
    status_normal: Color,
    status_search: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            pane_active: Color::Green,
            pane_inactive: Color::Gray,
            pane_search: Color::Yellow,
            list_highlight: Color::Yellow,
            now_border: Color::Magenta,
            status_normal: Color::Blue,
            status_search: Color::Yellow,
        }
    }
}

struct App {
    config: Config,
    tracks: Vec<Track>,
    library_track_count: usize,
    filtered: Vec<usize>,
    queue_filtered: Vec<usize>,
    queue: Vec<usize>,
    current_queue_pos: Option<usize>,
    library_state: ListState,
    queue_state: ListState,
    playlist_state: ListState,
    active_pane: Pane,
    left_view: LeftView,
    mode: Mode,
    search: String,
    search_target: Pane,
    command_input: String,
    status: String,
    should_quit: bool,
    pending_g: bool,
    pending_d: bool,
    pending_z: bool,
    editor_request: Option<PathBuf>,
    theme: Theme,
    snapshot_mtime: Option<SystemTime>,
    path_index: HashMap<String, usize>,
    path_index_dirty: bool,
    playlists_cache: Vec<PlaylistSummary>,
    playlists: PlaylistStore,
    active_playlist: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UiState {
    active_pane: Pane,
    left_view: LeftView,
    library_selected: Option<usize>,
    queue_selected: Option<usize>,
    playlist_selected: Option<usize>,
    active_playlist: String,
}

impl App {
    fn new(config: Config, tracks: Vec<Track>, playlists: PlaylistStore) -> Result<Self> {
        let filtered = (0..tracks.len()).collect::<Vec<_>>();
        let mut library_state = ListState::default();
        if !filtered.is_empty() {
            library_state.select(Some(0));
        }

        let mut app = Self {
            config,
            tracks,
            library_track_count: 0,
            filtered,
            queue_filtered: Vec::new(),
            queue: Vec::new(),
            current_queue_pos: None,
            library_state,
            queue_state: ListState::default(),
            playlist_state: ListState::default(),
            active_pane: Pane::Library,
            left_view: LeftView::Library,
            mode: Mode::Normal,
            search: String::new(),
            search_target: Pane::Library,
            command_input: String::new(),
            status: String::from("keys: hjkl gg G Ctrl-d/u Enter / p a dd J/K v c :q ZZ"),
            should_quit: false,
            pending_g: false,
            pending_d: false,
            pending_z: false,
            editor_request: None,
            theme: Theme::default(),
            snapshot_mtime: None,
            path_index: HashMap::new(),
            path_index_dirty: true,
            playlists_cache: Vec::new(),
            playlists,
            active_playlist: String::from("tui-queue"),
        };

        app.library_track_count = app.tracks.len();

        app.ensure_active_playlist_exists();
        app.apply_queue_filter();
        app.refresh_playlists();

        app.load_daemon_snapshot();
        if !app.queue.is_empty() {
            match app.sync_daemon_queue() {
                Ok(_) => app.status = String::from("restored queue to daemon"),
                Err(error) => app.status = format!("daemon restore failed ({error})"),
            }
        }
        app.restore_ui_state();
        Ok(app)
    }

    fn load_daemon_snapshot(&mut self) {
        let Ok(Some(snapshot)) = process::read_snapshot(&self.config.data_dir) else {
            return;
        };

        self.queue.clear();
        for path in snapshot.queue {
            let idx = self.track_index_for_path_or_insert(Path::new(&path));
            self.queue.push(idx);
        }

        self.current_queue_pos = snapshot.current.filter(|i| *i < self.queue.len());
        if self.current_queue_pos.is_some() {
            self.active_pane = Pane::Queue;
            self.queue_state.select(self.current_queue_pos);
            self.status = String::from("loaded running daemon queue");
        } else if !self.queue.is_empty() {
            self.active_pane = Pane::Queue;
            self.queue_state.select(Some(0));
            self.status = String::from("loaded daemon queue");
        }
        self.apply_queue_filter();
        self.garbage_collect_transient_tracks();
    }

    fn on_tick(&mut self) {
        if self.snapshot_changed() {
            self.sync_from_daemon_snapshot();
        }
        self.queue_filtered.retain(|pos| *pos < self.queue.len());
        self.normalize_selection();
    }

    fn refresh_playlists(&mut self) {
        self.playlists_cache = self.playlists.list().unwrap_or_default();
        if self.playlists_cache.is_empty() {
            self.playlist_state.select(None);
        } else {
            let idx = self
                .playlist_state
                .selected()
                .unwrap_or(0)
                .min(self.playlists_cache.len() - 1);
            self.playlist_state.select(Some(idx));
        }
    }

    fn ensure_active_playlist_exists(&mut self) {
        if self.playlists.create(&self.active_playlist).is_ok() {
            return;
        }
    }

    fn selected_playlist_name(&self) -> Option<&str> {
        let idx = self.playlist_state.selected()?;
        self.playlists_cache.get(idx).map(|p| p.name.as_str())
    }

    fn ui_state_path(&self) -> PathBuf {
        self.config.data_dir.join("ui-state.json")
    }

    fn restore_ui_state(&mut self) {
        let Ok(raw) = fs::read_to_string(self.ui_state_path()) else {
            return;
        };
        let Ok(state) = serde_json::from_str::<UiState>(&raw) else {
            return;
        };
        self.active_pane = state.active_pane;
        self.left_view = state.left_view;
        self.library_state.select(state.library_selected.filter(|i| *i < self.filtered.len()));
        self.queue_state.select(state.queue_selected.filter(|i| *i < self.queue_filtered.len()));
        self.playlist_state.select(state.playlist_selected.filter(|i| *i < self.playlists_cache.len()));
        if !state.active_playlist.trim().is_empty() {
            self.active_playlist = state.active_playlist;
        }
        self.normalize_selection();
    }

    fn persist_ui_state(&self) {
        let state = UiState {
            active_pane: self.active_pane,
            left_view: self.left_view,
            library_selected: self.library_state.selected(),
            queue_selected: self.queue_state.selected(),
            playlist_selected: self.playlist_state.selected(),
            active_playlist: self.active_playlist.clone(),
        };
        if let Ok(raw) = serde_json::to_string(&state) {
            let _ = fs::write(self.ui_state_path(), raw);
        }
    }

    fn snapshot_changed(&mut self) -> bool {
        let path = process::state_path(&self.config.data_dir);
        let Ok(meta) = fs::metadata(path) else {
            self.snapshot_mtime = None;
            return false;
        };
        let Ok(modified) = meta.modified() else {
            return true;
        };
        if self.snapshot_mtime == Some(modified) {
            return false;
        }
        self.snapshot_mtime = Some(modified);
        true
    }

    fn sync_from_daemon_snapshot(&mut self) {
        let Ok(Some(snapshot)) = process::read_snapshot(&self.config.data_dir) else {
            return;
        };

        let mut daemon_queue = Vec::new();
        for path in snapshot.queue {
            let idx = self.track_index_for_path_or_insert(Path::new(&path));
            daemon_queue.push(idx);
        }

        if daemon_queue == self.queue {
            self.current_queue_pos = snapshot.current.filter(|idx| *idx < self.queue.len());
        }
        self.garbage_collect_transient_tracks();
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        if self.mode == Mode::Search {
            match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.search.clear();
                    self.reset_filters();
                }
                KeyCode::Enter => {
                    self.mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    self.search.pop();
                    self.apply_search();
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        self.search.push(c);
                        self.apply_search();
                    }
                }
                _ => {}
            }
            self.normalize_selection();
            return Ok(());
        }

        if self.mode == Mode::Command {
            match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.command_input.clear();
                }
                KeyCode::Backspace => {
                    self.command_input.pop();
                }
                KeyCode::Enter => {
                    self.submit_command_mode();
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        self.command_input.push(c);
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        if self.pending_g {
            self.pending_g = false;
            if matches!(key.code, KeyCode::Char('g')) {
                self.select_first();
                return Ok(());
            }
        }
        if self.pending_d {
            self.pending_d = false;
            match key.code {
                KeyCode::Char('d') => {
                    if self.active_pane == Pane::Library && self.left_view == LeftView::Playlists {
                        let _ = self.delete_selected_playlist();
                    } else {
                        self.remove_selected_from_queue();
                    }
                    return Ok(());
                }
                KeyCode::Char('j') => {
                    self.delete_with_next();
                    return Ok(());
                }
                KeyCode::Char('k') => {
                    self.delete_with_previous();
                    return Ok(());
                }
                _ => {}
            }
        }
        if self.pending_z {
            self.pending_z = false;
            if matches!(key.code, KeyCode::Char('Z')) {
                self.should_quit = true;
                return Ok(());
            }
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.search.clear();
                self.reset_filters();
                self.status = String::from("search cleared");
            }
            KeyCode::Char(':') | KeyCode::Char(';') => {
                self.mode = Mode::Command;
                self.command_input.clear();
            }
            KeyCode::Char('Z') => self.pending_z = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Tab => self.toggle_pane(),
            KeyCode::Char('h') => self.active_pane = Pane::Library,
            KeyCode::Char('l') => self.active_pane = Pane::Queue,
            KeyCode::Char('p') if self.active_pane == Pane::Library => {
                self.left_view = match self.left_view {
                    LeftView::Library => LeftView::Playlists,
                    LeftView::Playlists => LeftView::Library,
                };
                self.refresh_playlists();
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.search.clear();
                self.search_target = self.active_pane;
                self.reset_filters();
                self.apply_search();
            }
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_previous(),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.select_last(),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.page_move(8)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.page_move(-8)
            }
            KeyCode::Char('d') => self.pending_d = true,
            KeyCode::Char('J') => self.move_queue_item(1),
            KeyCode::Char('K') => self.move_queue_item(-1),
            KeyCode::Enter => {
                if let Err(error) = self.activate_selected() {
                    self.status = error.to_string();
                }
            }
            KeyCode::Char('a') => self.append_selected_to_queue(),
            KeyCode::Char('c') if self.active_pane == Pane::Library && self.left_view == LeftView::Playlists => {
                self.mode = Mode::Command;
                self.command_input = String::from("plnew ");
            }
            KeyCode::Char('S') => self.save_queue()?,
            KeyCode::Char('v') => {
                if self.active_pane == Pane::Library && self.left_view == LeftView::Playlists {
                    self.open_selected_playlist_in_editor()?;
                } else {
                    self.open_queue_in_editor()?;
                }
            }
            _ => {}
        }
        self.normalize_selection();
        Ok(())
    }

    fn page_move(&mut self, delta: isize) {
        match self.active_pane {
            Pane::Library => match self.left_view {
                LeftView::Library => select_relative(&mut self.library_state, self.filtered.len(), delta),
                LeftView::Playlists => select_relative(&mut self.playlist_state, self.playlists_cache.len(), delta),
            },
            Pane::Queue => select_relative(&mut self.queue_state, self.queue_filtered.len(), delta),
        }
    }

    fn apply_library_filter(&mut self) {
        let query = self.search.to_lowercase();
        self.filtered = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| {
                query.is_empty() || track.short_path().to_lowercase().contains(&query)
            })
            .map(|(idx, _)| idx)
            .collect();
        self.library_state.select(if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        });
        self.status = format!("{} track(s) match", self.filtered.len());
        self.normalize_selection();
    }

    fn apply_queue_filter(&mut self) {
        let query = self.search.to_lowercase();
        self.queue_filtered = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, idx)| {
                query.is_empty()
                    || self.tracks[**idx]
                        .display_name()
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(pos, _)| pos)
            .collect();
        self.queue_state.select(if self.queue_filtered.is_empty() {
            None
        } else {
            Some(0)
        });
        self.status = format!("{} queue item(s) match", self.queue_filtered.len());
        self.normalize_selection();
    }

    fn apply_search(&mut self) {
        match self.search_target {
            Pane::Library => self.apply_library_filter(),
            Pane::Queue => self.apply_queue_filter(),
        }
    }

    fn reset_filters(&mut self) {
        self.filtered = (0..self.tracks.len()).collect();
        self.library_state.select(if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        });
        self.queue_filtered = (0..self.queue.len()).collect();
        if self.queue_filtered.is_empty() {
            self.queue_state.select(None);
        } else {
            let keep = self
                .current_queue_pos
                .filter(|idx| *idx < self.queue_filtered.len())
                .or(Some(0));
            self.queue_state.select(keep);
        }
        self.normalize_selection();
    }

    fn normalize_selection(&mut self) {
        let lib_len = self.filtered.len();
        match self.library_state.selected() {
            Some(idx) if idx >= lib_len => self.library_state.select(lib_len.checked_sub(1)),
            None if lib_len > 0 => self.library_state.select(Some(0)),
            _ => {}
        }

        let queue_len = self.queue_filtered.len();
        match self.queue_state.selected() {
            Some(idx) if idx >= queue_len => self.queue_state.select(queue_len.checked_sub(1)),
            None if queue_len > 0 => self.queue_state.select(Some(0)),
            _ => {}
        }

        let pl_len = self.playlists_cache.len();
        match self.playlist_state.selected() {
            Some(idx) if idx >= pl_len => self.playlist_state.select(pl_len.checked_sub(1)),
            None if pl_len > 0 => self.playlist_state.select(Some(0)),
            _ => {}
        }
    }

    fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Library => Pane::Queue,
            Pane::Queue => Pane::Library,
        };
    }

    fn selected_library_track(&self) -> Option<usize> {
        if self.left_view != LeftView::Library {
            return None;
        }
        self.library_state
            .selected()
            .and_then(|i| self.filtered.get(i).copied())
    }

    fn selected_queue_pos(&self) -> Option<usize> {
        let selected = self.queue_state.selected()?;
        self.queue_filtered.get(selected).copied()
    }

    fn select_next(&mut self) {
        match self.active_pane {
            Pane::Library => match self.left_view {
                LeftView::Library => select_relative(&mut self.library_state, self.filtered.len(), 1),
                LeftView::Playlists => select_relative(&mut self.playlist_state, self.playlists_cache.len(), 1),
            },
            Pane::Queue => select_relative(&mut self.queue_state, self.queue_filtered.len(), 1),
        }
    }

    fn select_previous(&mut self) {
        match self.active_pane {
            Pane::Library => match self.left_view {
                LeftView::Library => select_relative(&mut self.library_state, self.filtered.len(), -1),
                LeftView::Playlists => select_relative(&mut self.playlist_state, self.playlists_cache.len(), -1),
            },
            Pane::Queue => select_relative(&mut self.queue_state, self.queue_filtered.len(), -1),
        }
    }

    fn select_first(&mut self) {
        match self.active_pane {
            Pane::Library if self.left_view == LeftView::Library && !self.filtered.is_empty() => self.library_state.select(Some(0)),
            Pane::Library if self.left_view == LeftView::Playlists && !self.playlists_cache.is_empty() => self.playlist_state.select(Some(0)),
            Pane::Queue if !self.queue_filtered.is_empty() => self.queue_state.select(Some(0)),
            _ => {}
        }
    }

    fn select_last(&mut self) {
        match self.active_pane {
            Pane::Library if self.left_view == LeftView::Library && !self.filtered.is_empty() => {
                self.library_state.select(Some(self.filtered.len() - 1));
            }
            Pane::Library if self.left_view == LeftView::Playlists && !self.playlists_cache.is_empty() => {
                self.playlist_state.select(Some(self.playlists_cache.len() - 1));
            }
            Pane::Queue if !self.queue_filtered.is_empty() => {
                self.queue_state.select(Some(self.queue_filtered.len() - 1))
            }
            _ => {}
        }
    }

    fn activate_selected(&mut self) -> Result<()> {
        match self.active_pane {
            Pane::Library => {
                match self.left_view {
                    LeftView::Library => {
                        let Some(track_idx) = self.selected_library_track() else {
                            return Ok(());
                        };
                        self.play_single_track(track_idx)?;
                    }
                    LeftView::Playlists => {
                        let Some(name) = self.selected_playlist_name().map(str::to_string) else {
                            return Ok(());
                        };
                        self.active_playlist = name.clone();
                        self.reload_queue_from_playlist(&name)?;
                        self.current_queue_pos = if self.queue.is_empty() { None } else { Some(0) };
                        if !self.queue.is_empty() {
                            self.play_queue_pos(0)?;
                            self.active_pane = Pane::Queue;
                        }
                        self.status = format!("active playlist: {}", self.active_playlist);
                    }
                }
                Ok(())
            }
            Pane::Queue => {
                let Some(pos) = self.selected_queue_pos() else {
                    return Ok(());
                };
                self.play_queue_pos(pos)?;
                Ok(())
            }
        }
    }

    fn append_selected_to_queue(&mut self) {
        let Some(track_idx) = self.selected_library_track() else {
            return;
        };
        self.queue.push(track_idx);
        self.apply_queue_filter();
        if self.queue_state.selected().is_none() {
            self.queue_state.select(Some(0));
        }
        match self.sync_daemon_queue() {
            Ok(_) => {
                let _ = self.persist_active_playlist();
                self.status = format!("queued {}", self.tracks[track_idx].display_name());
            }
            Err(error) => self.status = format!("queue staged locally ({error})"),
        }
    }

    fn remove_selected_from_queue(&mut self) {
        if self.active_pane != Pane::Queue {
            return;
        }
        let Some(pos) = self.selected_queue_pos() else {
            return;
        };
        let selected_after = if self.queue.len() <= 1 {
            None
        } else {
            Some(pos.min(self.queue.len() - 2))
        };
        self.queue.remove(pos);
        if let Some(current) = self.current_queue_pos {
            self.current_queue_pos = if current == pos {
                if self.queue.is_empty() {
                    None
                } else {
                    Some(pos.min(self.queue.len() - 1))
                }
            } else if current > pos {
                Some(current - 1)
            } else {
                Some(current)
            };
        }
        self.apply_queue_filter();
        if let Some(pos) = selected_after {
            if let Some(filtered_pos) = self.queue_filtered.iter().position(|p| *p == pos) {
                self.queue_state.select(Some(filtered_pos));
            }
        }
        match self.sync_daemon_queue() {
            Ok(_) => {
                let _ = self.persist_active_playlist();
                self.status = String::from("removed item from queue");
            }
            Err(error) => self.status = format!("removed locally ({error})"),
        }
    }

    fn delete_with_next(&mut self) {
        if self.active_pane != Pane::Queue {
            return;
        }
        let Some(pos) = self.selected_queue_pos() else {
            return;
        };
        if self.queue.is_empty() {
            return;
        }
        let end = (pos + 1).min(self.queue.len() - 1);
        self.delete_queue_range(pos, end);
    }

    fn delete_with_previous(&mut self) {
        if self.active_pane != Pane::Queue {
            return;
        }
        let Some(pos) = self.selected_queue_pos() else {
            return;
        };
        if self.queue.is_empty() {
            return;
        }
        let start = pos.saturating_sub(1);
        self.delete_queue_range(start, pos);
    }

    fn delete_queue_range(&mut self, start: usize, end: usize) {
        if start >= self.queue.len() || end >= self.queue.len() || start > end {
            return;
        }

        let count = end - start + 1;
        self.queue.drain(start..=end);

        if let Some(current) = self.current_queue_pos {
            self.current_queue_pos = if (start..=end).contains(&current) {
                if self.queue.is_empty() {
                    None
                } else {
                    Some(start.min(self.queue.len() - 1))
                }
            } else if current > end {
                Some(current - count)
            } else {
                Some(current)
            };
        }

        self.apply_queue_filter();
        if !self.queue.is_empty() {
            let pos = start.min(self.queue.len() - 1);
            if let Some(filtered_pos) = self.queue_filtered.iter().position(|p| *p == pos) {
                self.queue_state.select(Some(filtered_pos));
            }
        }

        match self.sync_daemon_queue() {
            Ok(_) => {
                let _ = self.persist_active_playlist();
                self.status = String::from("removed queue range");
            }
            Err(error) => self.status = format!("removed locally ({error})"),
        }
    }

    fn move_queue_item(&mut self, delta: isize) {
        if self.active_pane != Pane::Queue {
            return;
        }
        let Some(pos) = self.selected_queue_pos() else {
            return;
        };
        let target = (pos as isize + delta).clamp(0, self.queue.len() as isize - 1) as usize;
        if target == pos {
            return;
        }

        if let Some(current) = self.current_queue_pos {
            self.current_queue_pos = if current == pos {
                Some(target)
            } else if pos < current && target >= current {
                Some(current - 1)
            } else if pos > current && target <= current {
                Some(current + 1)
            } else {
                Some(current)
            };
        }

        self.queue.swap(pos, target);
        self.apply_queue_filter();
        self.queue_state.select(Some(
            target.min(self.queue_filtered.len().saturating_sub(1)),
        ));
        match self.sync_daemon_queue() {
            Ok(_) => {
                let _ = self.persist_active_playlist();
                self.status = String::from("moved queue item");
            }
            Err(error) => self.status = format!("moved locally ({error})"),
        }
    }

    fn play_single_track(&mut self, track_idx: usize) -> Result<()> {
        let input = self.tracks[track_idx].path.to_string_lossy().to_string();
        let started = process::ensure_daemon_and_send_command(
            &self.config.data_dir,
            &process::DaemonCommand::Replace {
                inputs: vec![input],
            },
        )?;
        self.current_queue_pos = None;
        let track = &self.tracks[track_idx];
        self.status = if started {
            format!("playing single track {}", track.display_name())
        } else {
            format!("updated playback to {}", track.display_name())
        };
        Ok(())
    }

    fn play_queue_pos(&mut self, pos: usize) -> Result<()> {
        let Some(&track_idx) = self.queue.get(pos) else {
            return Ok(());
        };
        let inputs = self
            .queue
            .iter()
            .map(|idx| self.tracks[*idx].path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let started = process::ensure_daemon_and_send_command(
            &self.config.data_dir,
            &process::DaemonCommand::ReplaceFrom { inputs, start: pos },
        )?;
        let track = &self.tracks[track_idx];
        self.current_queue_pos = Some(pos);
        if self.search_target == Pane::Queue && self.mode == Mode::Search {
            if let Some(filtered_pos) = self.queue_filtered.iter().position(|p| *p == pos) {
                self.queue_state.select(Some(filtered_pos));
            }
        } else {
            self.queue_state.select(Some(pos));
        }
        process::wait_for_current_index(&self.config.data_dir, pos, Duration::from_secs(3))?;
        self.status = if started {
            format!("started background playback for {}", track.display_name())
        } else {
            format!("updated background playback to {}", track.display_name())
        };
        Ok(())
    }

    fn persist_active_playlist(&mut self) -> Result<()> {
        let files = self
            .queue
            .iter()
            .map(|idx| self.tracks[*idx].path.clone())
            .collect::<Vec<_>>();
        self.playlists.write(&self.active_playlist, &files)?;
        self.refresh_playlists();
        Ok(())
    }

    fn save_queue(&mut self) -> Result<()> {
        self.persist_active_playlist()?;
        self.status = format!("saved queue to playlist '{}'", self.active_playlist);
        Ok(())
    }

    fn open_queue_in_editor(&mut self) -> Result<()> {
        self.save_queue()?;
        self.editor_request = Some(self.playlists.file_path(&self.active_playlist)?);
        Ok(())
    }

    fn open_selected_playlist_in_editor(&mut self) -> Result<()> {
        let Some(name) = self.selected_playlist_name().map(str::to_string) else {
            return Ok(());
        };
        self.editor_request = Some(self.playlists.file_path(&name)?);
        Ok(())
    }

    fn delete_selected_playlist(&mut self) -> Result<()> {
        let Some(name) = self.selected_playlist_name().map(str::to_string) else {
            return Ok(());
        };
        self.playlists.delete(&name)?;
        self.refresh_playlists();
        self.status = format!("deleted playlist {name}");
        Ok(())
    }

    fn sync_daemon_queue(&self) -> Result<()> {
        let inputs = self
            .queue
            .iter()
            .map(|idx| self.tracks[*idx].path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let current = self.current_queue_pos.filter(|idx| *idx < inputs.len());
        process::ensure_daemon_and_send_command(
            &self.config.data_dir,
            &process::DaemonCommand::UpdateQueue { inputs, current },
        )?;
        Ok(())
    }

    fn reload_queue_from_playlist(&mut self, name: &str) -> Result<()> {
        let files = self.playlists.read(name)?;
        self.queue.clear();
        for path in files {
            let idx = self.track_index_for_path_or_insert(&path);
            self.queue.push(idx);
        }
        self.current_queue_pos = self.current_queue_pos.filter(|idx| *idx < self.queue.len());
        self.apply_queue_filter();
        self.garbage_collect_transient_tracks();
        Ok(())
    }

    fn garbage_collect_transient_tracks(&mut self) {
        if self.tracks.len() <= self.library_track_count {
            return;
        }

        let mut used_transient = std::collections::HashSet::<usize>::new();
        for idx in &self.queue {
            if *idx >= self.library_track_count {
                used_transient.insert(*idx);
            }
        }

        if used_transient.is_empty() {
            self.tracks.truncate(self.library_track_count);
            return;
        }

        let mut new_tracks = self.tracks[..self.library_track_count].to_vec();
        let mut remap = HashMap::<usize, usize>::new();

        let mut transient_sorted = used_transient.into_iter().collect::<Vec<_>>();
        transient_sorted.sort_unstable();
        for old_idx in transient_sorted {
            if let Some(track) = self.tracks.get(old_idx).cloned() {
                let new_idx = new_tracks.len();
                new_tracks.push(track);
                remap.insert(old_idx, new_idx);
            }
        }

        for idx in &mut self.queue {
            if *idx >= self.library_track_count {
                if let Some(mapped) = remap.get(idx).copied() {
                    *idx = mapped;
                }
            }
        }

        self.tracks = new_tracks;
        self.current_queue_pos = self.current_queue_pos.filter(|idx| *idx < self.queue.len());
        self.path_index_dirty = true;
    }

    fn track_index_for_path_or_insert(&mut self, path: &Path) -> usize {
        self.rebuild_path_index_if_needed();
        let key = path_key(path);
        if let Some(idx) = self.path_index.get(&key).copied() {
            return idx;
        }
        let idx = self.tracks.len();
        self.tracks.push(Track::from_path(path.to_path_buf()));
        self.path_index.insert(key, idx);
        idx
    }

    fn rebuild_path_index_if_needed(&mut self) {
        if !self.path_index_dirty {
            return;
        }
        self.path_index.clear();
        for (idx, track) in self.tracks.iter().enumerate() {
            self.path_index.insert(path_key(&track.path), idx);
        }
        self.path_index_dirty = false;
    }

    fn submit_command_mode(&mut self) {
        let cmd = self.command_input.trim().to_string();
        if let Some(name) = cmd
            .strip_prefix("plnew ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match self.playlists.create(name) {
                Ok(_) => {
                    self.refresh_playlists();
                    self.status = format!("created playlist {name}");
                }
                Err(error) => self.status = error.to_string(),
            }
        } else {
            match cmd.as_str() {
            "q" | "quit" => self.should_quit = true,
            "" => {}
            other => self.status = format!("unknown command: :{other}"),
            }
        }
        self.command_input.clear();
        self.mode = Mode::Normal;
    }
}

pub fn run(config: Config, tracks: Vec<Track>, playlists: PlaylistStore) -> Result<()> {
    let mut terminal = init_terminal()?;
    let mut app = App::new(config, tracks, playlists)?;
    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    let result: Result<()> = loop {
        terminal.draw(|frame| render(frame, &mut app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key)?;
            }
        }

        if let Some(path) = app.editor_request.take() {
            restore_terminal(&mut terminal)?;
            open_in_editor(&path)?;
            terminal = init_terminal()?;
            if path.file_stem().and_then(|s| s.to_str()) == Some(app.active_playlist.as_str()) {
                let active = app.active_playlist.clone();
                app.reload_queue_from_playlist(&active)?;
                match app.sync_daemon_queue() {
                    Ok(_) => app.status = String::from("queue updated from editor"),
                    Err(error) => app.status = format!("editor queue local only ({error})"),
                }
            } else {
                app.refresh_playlists();
                app.status = String::from("playlist updated from editor");
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break Ok(());
        }
    };

    app.persist_ui_state();
    restore_terminal(&mut terminal)?;
    result
}

fn open_in_editor(path: &PathBuf) -> Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| String::from("nvim"));
    let status = Command::new(editor).arg(path).status()?;
    if !status.success() {
        anyhow::bail!("editor exited with failure")
    }
    Ok(())
}

fn path_key(path: &Path) -> String {
    if let Ok(canon) = path.canonicalize() {
        return canon.to_string_lossy().to_string();
    }
    path.to_string_lossy().to_string()
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn render(frame: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(root[0]);

    render_library(frame, app, columns[0]);
    render_queue(frame, app, columns[1]);
    render_now_playing(frame, app, root[1]);
    render_help(frame, app, root[2]);
}

fn render_library(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let (title, items, state): (String, Vec<ListItem>, &mut ListState) = match app.left_view {
        LeftView::Library => {
            let items = app
                .filtered
                .iter()
                .map(|idx| {
                    let track = &app.tracks[*idx];
                    ListItem::new(Line::from(vec![Span::raw(track.display_name())]))
                })
                .collect::<Vec<_>>();
            let title = if !app.search.is_empty() && app.search_target == Pane::Library {
                format!("Library [{} track(s)]  filter: /{}", app.filtered.len(), app.search)
            } else {
                format!("Library {} track(s)", app.filtered.len())
            };
            (title, items, &mut app.library_state)
        }
        LeftView::Playlists => {
            let items = app
                .playlists_cache
                .iter()
                .map(|pl| ListItem::new(Line::from(vec![Span::raw(pl.name.clone())])))
                .collect::<Vec<_>>();
            (
                String::from("Playlists (p toggle, c create, dd delete, v edit)"),
                items,
                &mut app.playlist_state,
            )
        }
    };

    let active_color = if app.mode == Mode::Search && app.search_target == Pane::Library {
        app.theme.pane_search
    } else {
        app.theme.pane_active
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.active_pane == Pane::Library {
            Style::default()
                .fg(active_color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.pane_inactive)
        });

    let list = List::new(items)
        .block(block)
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(app.theme.list_highlight)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, state);
}

fn render_queue(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let items = app
        .queue_filtered
        .iter()
        .filter_map(|pos| {
            let idx = *app.queue.get(*pos)?;
            let marker = if Some(*pos) == app.current_queue_pos {
                "♪ "
            } else {
                "  "
            };
            Some(ListItem::new(Line::from(format!(
                "{marker}{}",
                app.tracks[idx].display_name()
            ))))
        })
        .collect::<Vec<_>>();

    let title = if !app.search.is_empty() && app.search_target == Pane::Queue {
        format!(
            "Queue [{} item(s)]  [{}] filter: /{}",
            app.queue_filtered.len(),
            app.active_playlist,
            app.search
        )
    } else {
        format!("Queue {} item(s) [{}]", app.queue.len(), app.active_playlist)
    };

    let active_color = if app.mode == Mode::Search && app.search_target == Pane::Queue {
        app.theme.pane_search
    } else {
        app.theme.pane_active
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.active_pane == Pane::Queue {
            Style::default()
                .fg(active_color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.pane_inactive)
        });

    let list = List::new(items)
        .block(block)
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(app.theme.list_highlight)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut app.queue_state);
}

fn render_now_playing(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let now = app
        .current_queue_pos
        .and_then(|pos| app.queue.get(pos))
        .map(|idx| app.tracks[*idx].display_name())
        .unwrap_or_else(|| String::from("nothing playing"));

    let text = vec![
        Line::from(format!("{now}")),
        Line::from(format!(
            "background mode  music: {}",
            app.config.music_dir.display()
        )),
    ];
    let paragraph = Paragraph::new(text).block(
        Block::default()
            .title("Now")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.now_border)),
    );
    frame.render_widget(paragraph, area);
}

fn render_help(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let prefix = if app.mode == Mode::Command {
        format!(":{}", app.command_input)
    } else if app.mode == Mode::Search {
        let target = match app.search_target {
            Pane::Library => "LIB",
            Pane::Queue => "QUEUE",
        };
        if app.search.is_empty() {
            format!("SEARCH[{target}]")
        } else {
            format!("SEARCH[{target}] /{}", app.search)
        }
    } else {
        String::from("NORMAL")
    };
    let text = format!("{prefix} | {}", app.status);
    let style = if app.mode == Mode::Search {
        Style::default()
            .fg(app.theme.status_search)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.status_normal)
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
}

fn select_relative(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, len as isize - 1) as usize;
    state.select(Some(next));
}
