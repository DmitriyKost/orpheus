use std::{sync::mpsc, time::Duration};

use crate::library::Track;

// MPRIS (Linux) and MPNowPlayingInfoCenter / MPRemoteCommandCenter (macOS) share the
// same `souvlaki` API, so the active session logic is identical on both platforms.
#[derive(Debug, Clone, Copy)]
pub enum MediaControlAction {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
}

/// The metadata currently published to the OS, kept so it can be re-asserted on macOS
/// (see `refresh_tick`).
#[cfg(any(target_os = "linux", target_os = "macos"))]
struct NowPlaying {
    artist: Option<String>,
    title: String,
    duration: Option<Duration>,
    playing: bool,
}

pub struct MediaSession {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    controls: Option<souvlaki::MediaControls>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    events_rx: Option<mpsc::Receiver<MediaControlAction>>,
    #[cfg(target_os = "macos")]
    current: Option<NowPlaying>,
    // Elapsed playback time tracking for the live position published on macOS. macOS keeps
    // the Now Playing tile alive (and re-evaluates which app owns it) only while the info
    // dict keeps changing, so the daemon re-asserts a continuously advancing elapsed time
    // (see `refresh_tick`). `elapsed_base` is time accumulated before the current playing
    // segment; `segment_start` is when the current segment began, or `None` while paused.
    #[cfg(target_os = "macos")]
    elapsed_base: Duration,
    #[cfg(target_os = "macos")]
    segment_start: Option<std::time::Instant>,
}

impl MediaSession {
    pub fn new() -> Self {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let config = souvlaki::PlatformConfig {
                dbus_name: "orpheus",
                display_name: "Orpheus",
                // Only used on Windows, where a window handle is required.
                hwnd: None,
            };

            let controls = match souvlaki::MediaControls::new(config) {
                Ok(mut controls) => {
                    let (events_tx, events_rx) = mpsc::channel();
                    let attached = controls.attach(move |event| {
                        let action = match event {
                            souvlaki::MediaControlEvent::Play => Some(MediaControlAction::Play),
                            souvlaki::MediaControlEvent::Pause => Some(MediaControlAction::Pause),
                            souvlaki::MediaControlEvent::Toggle => Some(MediaControlAction::PlayPause),
                            souvlaki::MediaControlEvent::Next => Some(MediaControlAction::Next),
                            souvlaki::MediaControlEvent::Previous => Some(MediaControlAction::Previous),
                            souvlaki::MediaControlEvent::Stop => Some(MediaControlAction::Stop),
                            _ => None,
                        };
                        if let Some(action) = action {
                            let _ = events_tx.send(action);
                        }
                    });

                    if let Err(error) = attached {
                        eprintln!("media session attach failed: {error}");
                        return Self {
                            controls: None,
                            events_rx: None,
                            #[cfg(target_os = "macos")]
                            current: None,
                            #[cfg(target_os = "macos")]
                            elapsed_base: Duration::ZERO,
                            #[cfg(target_os = "macos")]
                            segment_start: None,
                        };
                    }

                    return Self {
                        controls: Some(controls),
                        events_rx: Some(events_rx),
                        #[cfg(target_os = "macos")]
                        current: None,
                        #[cfg(target_os = "macos")]
                        elapsed_base: Duration::ZERO,
                        #[cfg(target_os = "macos")]
                        segment_start: None,
                    };
                }
                Err(error) => {
                    eprintln!("media session setup failed: {error}");
                    None
                }
            };

            Self {
                controls,
                events_rx: None,
                #[cfg(target_os = "macos")]
                current: None,
                #[cfg(target_os = "macos")]
                elapsed_base: Duration::ZERO,
                #[cfg(target_os = "macos")]
                segment_start: None,
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Self {}
        }
    }

    pub fn track_started(&mut self, track: &Track, duration: Option<Duration>) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let display = track.display_name();
            let (artist, title) = split_artist_title(&display);
            let now_playing = NowPlaying {
                artist: artist.map(str::to_string),
                title,
                duration,
                playing: true,
            };
            #[cfg(target_os = "macos")]
            {
                // Force a clean Stopped -> Playing transition on a track *change*. macOS
                // re-evaluates which app owns the Now Playing tile on a state transition; a
                // Playing -> Playing metadata swap is often ignored, which drops the tile
                // when switching tracks while one is already playing.
                if self.current.is_some() {
                    if let Some(controls) = self.controls.as_mut() {
                        let _ = controls.set_playback(souvlaki::MediaPlayback::Stopped);
                    }
                }
                self.elapsed_base = Duration::ZERO;
                self.segment_start = Some(std::time::Instant::now());
            }
            self.apply(&now_playing);
            #[cfg(target_os = "macos")]
            {
                self.current = Some(now_playing);
            }
        }
    }

    pub fn finished(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(controls) = self.controls.as_mut() {
            let _ = controls.set_playback(souvlaki::MediaPlayback::Stopped);
        }
        #[cfg(target_os = "macos")]
        {
            self.current = None;
            self.elapsed_base = Duration::ZERO;
            self.segment_start = None;
        }
    }

    pub fn set_paused(&mut self) {
        #[cfg(target_os = "macos")]
        {
            // Freeze elapsed time at the pause point, then re-publish as paused.
            if let Some(start) = self.segment_start.take() {
                self.elapsed_base += start.elapsed();
            }
            if let Some(now_playing) = self.current.as_mut() {
                now_playing.playing = false;
            }
            self.reassert();
        }
        #[cfg(all(target_os = "linux", not(target_os = "macos")))]
        if let Some(controls) = self.controls.as_mut() {
            let _ = controls.set_playback(souvlaki::MediaPlayback::Paused { progress: None });
        }
    }

    pub fn set_playing(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if self.segment_start.is_none() {
                self.segment_start = Some(std::time::Instant::now());
            }
            if let Some(now_playing) = self.current.as_mut() {
                now_playing.playing = true;
            }
            self.reassert();
        }
        #[cfg(all(target_os = "linux", not(target_os = "macos")))]
        if let Some(controls) = self.controls.as_mut() {
            let _ = controls.set_playback(souvlaki::MediaPlayback::Playing { progress: None });
        }
    }

    /// Elapsed playback time of the current track, accounting for paused segments.
    #[cfg(target_os = "macos")]
    fn elapsed(&self) -> Duration {
        self.elapsed_base + self.segment_start.map(|start| start.elapsed()).unwrap_or_default()
    }

    /// Publish the given metadata + playback state to the OS media session.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn apply(&mut self, now_playing: &NowPlaying) {
        // On macOS publish a live elapsed position so each re-assert is a *changed* info
        // dict, which keeps the tile registered and the progress bar moving. Linux (MPRIS)
        // keeps its previous behaviour of not reporting a position.
        #[cfg(target_os = "macos")]
        let progress = Some(souvlaki::MediaPosition(self.elapsed()));
        #[cfg(not(target_os = "macos"))]
        let progress: Option<souvlaki::MediaPosition> = None;

        let Some(controls) = self.controls.as_mut() else {
            return;
        };
        let metadata = souvlaki::MediaMetadata {
            title: Some(now_playing.title.as_str()),
            album: None,
            artist: now_playing.artist.as_deref(),
            cover_url: None,
            duration: now_playing.duration,
        };
        let _ = controls.set_metadata(metadata);
        let playback = if now_playing.playing {
            souvlaki::MediaPlayback::Playing { progress }
        } else {
            souvlaki::MediaPlayback::Paused { progress }
        };
        let _ = controls.set_playback(playback);
    }

    /// Re-publish the current track's info (with the latest elapsed time).
    #[cfg(target_os = "macos")]
    fn reassert(&mut self) {
        if let Some(now_playing) = self.current.take() {
            self.apply(&now_playing);
            self.current = Some(now_playing);
        }
    }

    /// Called from the daemon's macOS run-loop pump on every tick. Re-publishes the current
    /// track so the advancing elapsed time keeps the Now Playing tile alive and registered.
    /// No-op when nothing is playing.
    #[cfg(target_os = "macos")]
    pub fn refresh_tick(&mut self) {
        self.reassert();
    }

    pub fn take_events_rx(&mut self) -> Option<mpsc::Receiver<MediaControlAction>> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            self.events_rx.take()
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            None
        }
    }
}

/// Pump the platform event loop so media-key / remote-command events get delivered.
///
/// On macOS, `souvlaki`'s `MPRemoteCommandCenter` handlers are invoked by the Core
/// Foundation run loop on the main thread; if that run loop never runs, the play/pause
/// and next/previous media keys are silently dropped. The daemon calls this from its
/// main loop in place of a blocking channel wait. On Linux (MPRIS) the events arrive
/// over D-Bus on souvlaki's own thread, so this blocks on the channel instead.
#[cfg(target_os = "macos")]
pub fn pump_events(timeout: Duration) {
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    let _ = CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, timeout, false);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn split_artist_title(display: &str) -> (Option<&str>, String) {
    if let Some((artist, title)) = display.split_once(" - ") {
        return (Some(artist), title.to_string());
    }
    (None, display.to_string())
}
