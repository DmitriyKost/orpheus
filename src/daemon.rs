use anyhow::Result;
use std::{
    fs,
    io::ErrorKind,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::mpsc::{self},
    thread,
    time::Duration,
};

use crate::{
    audio::{duration, NativePlayer},
    config::Config,
    library::{Library, Track},
    media::{MediaControlAction, MediaSession},
    playlist::PlaylistStore,
    process::{DaemonCommand, DaemonSnapshot},
};

pub fn run(config: Config, library: Library, playlists: PlaylistStore) -> Result<()> {
    let socket = crate::process::socket_path(&config.data_dir);
    if socket.exists() {
        match UnixStream::connect(&socket) {
            Ok(_) => anyhow::bail!("daemon is already running"),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound
                        | ErrorKind::ConnectionRefused
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::AddrNotAvailable
                ) =>
            {
                fs::remove_file(&socket)?;
            }
            Err(error) => anyhow::bail!("cannot safely replace daemon socket: {error}"),
        }
    }
    let listener = UnixListener::bind(&socket)?;
    let (event_tx, event_rx) = mpsc::channel::<CoreEvent>();
    let request_tx = event_tx.clone();
    let _socket_thread = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue; };
            if let Err(error) = handle_stream(&request_tx, stream) {
                eprintln!("daemon command error: {error}");
            }
        }
    });

    let mut state = DaemonState::new(config.data_dir.clone(), library, playlists)?;
    if let Some(media_rx) = state.media.take_events_rx() {
        let media_tx = event_tx.clone();
        let _media_thread = thread::spawn(move || {
            while let Ok(event) = media_rx.recv() {
                let _ = media_tx.send(CoreEvent::Media(event));
            }
        });
    }

    let result = loop {
        // On macOS the media-key handlers fire from the Core Foundation run loop on this
        // (main) thread, so we pump it for a slice instead of blocking on the channel, then
        // drain any IPC/media events that queued up. On Linux MPRIS events arrive on
        // souvlaki's own thread, so a plain blocking wait is enough.
        #[cfg(target_os = "macos")]
        {
            crate::media::pump_events(Duration::from_millis(200));
            while let Ok(event) = event_rx.try_recv() {
                handle_event(&mut state, event);
            }
            state.media.refresh_tick();
        }

        #[cfg(not(target_os = "macos"))]
        match event_rx.recv_timeout(Duration::from_millis(300)) {
            Ok(event) => {
                handle_event(&mut state, event);
                while let Ok(event) = event_rx.try_recv() {
                    handle_event(&mut state, event);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        }

        if state.on_audio_tick()? {
            break Ok(());
        }

    };

    drop(event_rx);
    let _ = fs::remove_file(&socket);
    result
}

enum CoreEvent {
    Request(DaemonRequest),
    Media(MediaControlAction),
}

struct DaemonRequest {
    command: DaemonCommand,
    response_tx: mpsc::Sender<String>,
}

fn handle_stream(request_tx: &mpsc::Sender<CoreEvent>, stream: UnixStream) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let command: DaemonCommand = serde_json::from_str(line.trim())?;
    let (response_tx, response_rx) = mpsc::channel();
    request_tx.send(CoreEvent::Request(DaemonRequest { command, response_tx }))?;
    let response = response_rx.recv()?;

    let mut stream = reader.into_inner();
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn handle_event(state: &mut DaemonState, event: CoreEvent) {
    match event {
        CoreEvent::Request(request) => {
            let response = match state.handle_command(request.command) {
                Ok(_) => "ok".to_string(),
                Err(err) => err.to_string(),
            };
            let _ = request.response_tx.send(response);
        }
        CoreEvent::Media(event) => {
            if let Err(error) = state.handle_media_event(event) {
                eprintln!("daemon media event error: {error}");
            }
        }
    }
}

struct DaemonState {
    data_dir: PathBuf,
    player: NativePlayer,
    media: MediaSession,
    library: Library,
    playlists: PlaylistStore,
    queue: Vec<Track>,
    current: Option<usize>,
    // Path of the track currently loaded in the player, tracked separately from `current`
    // so the persisted snapshot reflects real playback even when the playing track is no
    // longer in `queue` (see the `UpdateQueue { current: None }` case below).
    playing_path: Option<String>,
    should_stop: bool,
}

impl DaemonState {
    fn new(data_dir: PathBuf, library: Library, playlists: PlaylistStore) -> Result<Self> {
        let mut state = Self {
            data_dir,
            player: NativePlayer::new()?,
            media: MediaSession::new(),
            library,
            playlists,
            queue: Vec::new(),
            current: None,
            playing_path: None,
            should_stop: false,
        };

        state.restore_snapshot_state()?;
        Ok(state)
    }

    fn handle_command(&mut self, command: DaemonCommand) -> Result<()> {
        match command {
            DaemonCommand::Replace { inputs } => {
                let tracks = resolve_inputs(&self.library, &self.playlists, &inputs)?;
                if tracks.is_empty() {
                    anyhow::bail!("nothing to play");
                }
                self.queue = tracks;
                self.current = Some(0);
                self.play_current()?;
                self.persist_state()?;
            }
            DaemonCommand::ReplaceFrom { inputs, start } => {
                let tracks = resolve_inputs(&self.library, &self.playlists, &inputs)?;
                if tracks.is_empty() {
                    anyhow::bail!("nothing to play");
                }
                let start = start.min(tracks.len() - 1);
                self.queue = tracks;
                self.current = Some(start);
                self.play_current()?;
                self.persist_state()?;
            }
            DaemonCommand::UpdateQueue { inputs, current } => {
                let was_paused = self.player.is_paused();
                let old_current_path = self
                    .current
                    .and_then(|idx| self.queue.get(idx))
                    .map(|track| track.path.clone());
                let tracks = inputs
                    .into_iter()
                    .map(|path| Track::from_path(std::path::PathBuf::from(path)))
                    .collect::<Vec<_>>();
                self.queue = tracks;
                self.current = current.filter(|idx| *idx < self.queue.len());

                if self.queue.is_empty() {
                    self.player.stop();
                    self.playing_path = None;
                    self.media.finished();
                    self.persist_state()?;
                    return Ok(());
                }

                if self.current.is_none() {
                    // The queue was edited while the playing track isn't part of it (a
                    // standalone library pick played over an active playlist). Keep whatever
                    // is currently playing and just record the updated queue; if nothing is
                    // playing it simply stays stopped. Playback is only interrupted on an
                    // explicit Stop or when the queue becomes empty (handled above).
                    self.persist_state()?;
                    return Ok(());
                }

                let new_current_path = self
                    .current
                    .and_then(|idx| self.queue.get(idx))
                    .map(|track| track.path.clone());
                let current_changed = old_current_path != new_current_path;

                if current_changed || self.player.is_empty() {
                    self.ensure_current_loaded_paused(current_changed)?;
                    if !was_paused {
                        self.player.resume();
                        self.media.set_playing();
                    }
                }
                self.persist_state()?;
            }
            DaemonCommand::Stop => {
                self.player.stop();
                self.current = None;
                self.playing_path = None;
                self.media.finished();
                self.should_stop = true;
                self.persist_state()?;
            }
        }
        Ok(())
    }

    fn on_audio_tick(&mut self) -> Result<bool> {
        if let Some(current) = self.current {
            if self.player.is_empty() && !self.player.is_paused() {
                if current + 1 < self.queue.len() {
                    self.current = Some(current + 1);
                    self.play_current()?;
                } else {
                    self.current = None;
                    self.playing_path = None;
                    self.media.finished();
                    self.persist_state()?;
                }
            }
        }

        Ok(self.should_stop)
    }

    fn play_current(&mut self) -> Result<()> {
        let Some(idx) = self.current else { return Ok(()); };
        let path = self.queue[idx].path.clone();
        // Start audio before publishing metadata: macOS only accepts Now Playing info
        // once the process is the active audio app, which CoreAudio output establishes.
        self.player.play(&path)?;
        self.playing_path = Some(path.to_string_lossy().to_string());
        let track = &self.queue[idx];
        self.media.track_started(track, duration(&track.path));
        self.persist_state()?;
        Ok(())
    }

    fn handle_media_event(&mut self, event: MediaControlAction) -> Result<()> {
        match event {
            MediaControlAction::PlayPause => {
                self.player.toggle_pause();
                if self.player.is_paused() {
                    self.media.set_paused();
                } else {
                    self.media.set_playing();
                }
            }
            MediaControlAction::Play => {
                if self.player.is_empty() {
                    self.ensure_current_loaded_paused(false)?;
                }
                self.player.resume();
                self.media.set_playing();
            }
            MediaControlAction::Pause => {
                self.player.pause();
                self.media.set_paused();
            }
            MediaControlAction::Next => {
                if let Some(current) = self.current {
                    if current + 1 < self.queue.len() {
                        self.current = Some(current + 1);
                        self.play_current()?;
                    }
                }
            }
            MediaControlAction::Previous => {
                if let Some(current) = self.current {
                    let prev = current.saturating_sub(1);
                    self.current = Some(prev);
                    self.play_current()?;
                }
            }
            MediaControlAction::Stop => {
                self.player.stop();
                self.current = None;
                self.playing_path = None;
                self.media.finished();
                self.persist_state()?;
            }
        }
        Ok(())
    }

    fn persist_state(&self) -> Result<()> {
        let snapshot = DaemonSnapshot {
            queue: self
                .queue
                .iter()
                .map(|track| track.path.to_string_lossy().to_string())
                .collect(),
            current: self.current,
            playing: self.playing_path.clone(),
        };
        crate::process::write_snapshot(&self.data_dir, &snapshot)
    }

    fn restore_snapshot_state(&mut self) -> Result<()> {
        let Some(snapshot) = crate::process::read_snapshot(&self.data_dir)? else {
            return Ok(());
        };

        self.queue = snapshot
            .queue
            .into_iter()
            .map(|path| Track::from_path(std::path::PathBuf::from(path)))
            .collect();
        self.current = snapshot
            .current
            .filter(|idx| *idx < self.queue.len())
            .or_else(|| (!self.queue.is_empty()).then_some(0));
        self.ensure_current_loaded_paused(false)?;
        Ok(())
    }

    fn ensure_current_loaded_paused(&mut self, force_reload: bool) -> Result<()> {
        if !force_reload && !self.player.is_empty() {
            return Ok(());
        }
        let Some(idx) = self.current else { return Ok(()); };
        let Some(path) = self.queue.get(idx).map(|track| track.path.clone()) else { return Ok(()); };
        self.player.play(&path)?;
        self.player.pause();
        self.playing_path = Some(path.to_string_lossy().to_string());
        let track = &self.queue[idx];
        self.media.track_started(track, duration(&track.path));
        self.media.set_paused();
        Ok(())
    }
}

fn resolve_inputs(library: &Library, playlists: &PlaylistStore, inputs: &[String]) -> Result<Vec<Track>> {
    if inputs.is_empty() {
        return Ok(library.scan()?);
    }

    let mut tracks = Vec::new();
    for input in inputs {
        tracks.extend(resolve_one_input(playlists, input)?);
    }
    Ok(tracks)
}

fn resolve_one_input(playlists: &PlaylistStore, input: &str) -> Result<Vec<Track>> {
    let input_path = std::path::PathBuf::from(input);
    if input_path.exists() {
        if input_path.is_dir() {
            return Ok(Library::new(input_path).scan()?);
        }
        return Ok(vec![Track::from_path(input_path)]);
    }

    let playlist_tracks = playlists.read(input)?;
    Ok(playlist_tracks.into_iter().map(Track::from_path).collect())
}
