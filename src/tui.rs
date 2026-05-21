use std::collections::HashMap;
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
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

use crate::{config::Config, library::Track, playlist::PlaylistStore, process};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Library,
    Queue,
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
    filtered: Vec<usize>,
    queue_filtered: Vec<usize>,
    queue: Vec<usize>,
    current_queue_pos: Option<usize>,
    library_state: ListState,
    queue_state: ListState,
    active_pane: Pane,
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
    playlists: PlaylistStore,
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
            filtered,
            queue_filtered: Vec::new(),
            queue: Vec::new(),
            current_queue_pos: None,
            library_state,
            queue_state: ListState::default(),
            active_pane: Pane::Library,
            mode: Mode::Normal,
            search: String::new(),
            search_target: Pane::Library,
            command_input: String::new(),
            status: String::from("nvim keys: hjkl gg G Ctrl-d/u dd J/K Enter / v :q ZZ"),
            should_quit: false,
            pending_g: false,
            pending_d: false,
            pending_z: false,
            editor_request: None,
            theme: Theme::default(),
            playlists,
        };

        app.apply_queue_filter();

        app.load_daemon_snapshot();
        if !app.queue.is_empty() {
            match app.sync_daemon_queue() {
                Ok(_) => app.status = String::from("restored queue to daemon"),
                Err(error) => app.status = format!("daemon restore failed ({error})"),
            }
        }
        Ok(app)
    }

    fn load_daemon_snapshot(&mut self) {
        let Ok(Some(snapshot)) = process::read_snapshot(&self.config.data_dir) else {
            return;
        };

        let mut by_path = self
            .tracks
            .iter()
            .enumerate()
            .map(|(idx, track)| (path_key(&track.path), idx))
            .collect::<HashMap<_, _>>();

        self.queue.clear();
        for path in snapshot.queue {
            let key = path_key(Path::new(&path));
            let idx = if let Some(idx) = by_path.get(&key).copied() {
                idx
            } else {
                let idx = self.tracks.len();
                self.tracks.push(Track::from_path(PathBuf::from(&path)));
                by_path.insert(key, idx);
                idx
            };
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
    }

    fn on_tick(&mut self) {
        self.sync_from_daemon_snapshot();
        self.queue_filtered.retain(|pos| *pos < self.queue.len());
        self.normalize_selection();
    }

    fn sync_from_daemon_snapshot(&mut self) {
        let Ok(Some(snapshot)) = process::read_snapshot(&self.config.data_dir) else {
            return;
        };

        let mut by_path = self
            .tracks
            .iter()
            .enumerate()
            .map(|(idx, track)| (path_key(&track.path), idx))
            .collect::<HashMap<_, _>>();

        let mut daemon_queue = Vec::new();
        for path in snapshot.queue {
            let key = path_key(Path::new(&path));
            let idx = if let Some(idx) = by_path.get(&key).copied() {
                idx
            } else {
                let idx = self.tracks.len();
                self.tracks.push(Track::from_path(PathBuf::from(&path)));
                by_path.insert(key, idx);
                idx
            };
            daemon_queue.push(idx);
        }

        if daemon_queue != self.queue {
            self.queue = daemon_queue;
            if self.mode != Mode::Search || self.search_target != Pane::Queue {
                self.search.clear();
                self.reset_filters();
            } else {
                self.apply_queue_filter();
            }
        }

        self.current_queue_pos = snapshot.current.filter(|idx| *idx < self.queue.len());
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
            if matches!(key.code, KeyCode::Char('d')) {
                self.remove_selected_from_queue();
                return Ok(());
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
            KeyCode::Char('S') => self.save_queue()?,
            KeyCode::Char('v') => self.open_queue_in_editor()?,
            _ => {}
        }
        self.normalize_selection();
        Ok(())
    }

    fn page_move(&mut self, delta: isize) {
        match self.active_pane {
            Pane::Library => select_relative(&mut self.library_state, self.filtered.len(), delta),
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
    }

    fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Library => Pane::Queue,
            Pane::Queue => Pane::Library,
        };
    }

    fn selected_library_track(&self) -> Option<usize> {
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
            Pane::Library => select_relative(&mut self.library_state, self.filtered.len(), 1),
            Pane::Queue => select_relative(&mut self.queue_state, self.queue_filtered.len(), 1),
        }
    }

    fn select_previous(&mut self) {
        match self.active_pane {
            Pane::Library => select_relative(&mut self.library_state, self.filtered.len(), -1),
            Pane::Queue => select_relative(&mut self.queue_state, self.queue_filtered.len(), -1),
        }
    }

    fn select_first(&mut self) {
        match self.active_pane {
            Pane::Library if !self.filtered.is_empty() => self.library_state.select(Some(0)),
            Pane::Queue if !self.queue_filtered.is_empty() => self.queue_state.select(Some(0)),
            _ => {}
        }
    }

    fn select_last(&mut self) {
        match self.active_pane {
            Pane::Library if !self.filtered.is_empty() => {
                self.library_state.select(Some(self.filtered.len() - 1))
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
                let Some(selected) = self.library_state.selected() else {
                    return Ok(());
                };
                self.queue = self.filtered.iter().skip(selected).copied().collect();
                self.current_queue_pos = if self.queue.is_empty() { None } else { Some(0) };
                self.search.clear();
                self.mode = Mode::Normal;
                self.reset_filters();
                self.queue_state.select(if self.queue.is_empty() { None } else { Some(0) });
                self.play_queue_pos(0)?;
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
            Ok(_) => self.status = format!("queued {}", self.tracks[track_idx].display_name()),
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
        match self.sync_daemon_queue() {
            Ok(_) => self.status = String::from("removed item from queue"),
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
        self.queue.swap(pos, target);
        self.apply_queue_filter();
        self.queue_state.select(Some(
            target.min(self.queue_filtered.len().saturating_sub(1)),
        ));
        match self.sync_daemon_queue() {
            Ok(_) => self.status = String::from("moved queue item"),
            Err(error) => self.status = format!("moved locally ({error})"),
        }
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

    fn save_queue(&mut self) -> Result<()> {
        let files = self
            .queue
            .iter()
            .map(|idx| self.tracks[*idx].path.clone())
            .collect::<Vec<_>>();
        self.playlists.write("tui-queue", &files)?;
        self.status = String::from("saved queue as playlist 'tui-queue'");
        Ok(())
    }

    fn open_queue_in_editor(&mut self) -> Result<()> {
        self.save_queue()?;
        self.editor_request = Some(self.playlists.file_path("tui-queue"));
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
        let mut by_path = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (path_key(&t.path), i))
            .collect::<HashMap<_, _>>();

        self.queue.clear();
        for path in files {
            let key = path_key(&path);
            let idx = if let Some(idx) = by_path.get(&key).copied() {
                idx
            } else {
                let idx = self.tracks.len();
                self.tracks.push(Track::from_path(path.clone()));
                by_path.insert(key, idx);
                idx
            };
            self.queue.push(idx);
        }
        self.current_queue_pos = self.current_queue_pos.filter(|idx| *idx < self.queue.len());
        self.apply_queue_filter();
        Ok(())
    }

    fn submit_command_mode(&mut self) {
        let cmd = self.command_input.trim();
        match cmd {
            "q" | "quit" => self.should_quit = true,
            "" => {}
            other => self.status = format!("unknown command: :{other}"),
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
            app.reload_queue_from_playlist("tui-queue")?;
            match app.sync_daemon_queue() {
                Ok(_) => app.status = String::from("queue updated from editor"),
                Err(error) => app.status = format!("editor queue local only ({error})"),
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
    let items = app
        .filtered
        .iter()
        .map(|idx| {
            let track = &app.tracks[*idx];
            ListItem::new(Line::from(vec![Span::raw(track.display_name())]))
        })
        .collect::<Vec<_>>();

    let title = if !app.search.is_empty() && app.search_target == Pane::Library {
        format!(
            "Library [{} track(s)]  filter: /{}",
            app.filtered.len(),
            app.search
        )
    } else {
        format!("Library {} track(s)", app.filtered.len())
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
    frame.render_stateful_widget(list, area, &mut app.library_state);
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
            "Queue [{} item(s)]  filter: /{}",
            app.queue_filtered.len(),
            app.search
        )
    } else {
        format!("Queue {} item(s)", app.queue.len())
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
