use std::{sync::mpsc, time::Duration};

use crate::library::Track;

#[derive(Debug, Clone, Copy)]
pub enum MediaControlAction {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
}

pub struct MediaSession {
    #[cfg(target_os = "linux")]
    controls: Option<souvlaki::MediaControls>,
    #[cfg(target_os = "linux")]
    events_rx: Option<mpsc::Receiver<MediaControlAction>>,
}

impl MediaSession {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        {
            let config = souvlaki::PlatformConfig {
                dbus_name: "orpheus",
                display_name: "Orpheus",
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
                        eprintln!("MPRIS attach failed: {error}");
                        return Self {
                            controls: None,
                            events_rx: None,
                        };
                    }

                    return Self {
                        controls: Some(controls),
                        events_rx: Some(events_rx),
                    };
                }
                Err(error) => {
                    eprintln!("MPRIS setup failed: {error}");
                    None
                }
            };

            Self {
                controls,
                events_rx: None,
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            Self {}
        }
    }

    pub fn track_started(&mut self, track: &Track, duration: Option<Duration>) {
        #[cfg(target_os = "linux")]
        if let Some(controls) = self.controls.as_mut() {
            let display = track.display_name();
            let (artist, title) = split_artist_title(&display);
            let metadata = souvlaki::MediaMetadata {
                title: Some(title.as_str()),
                album: None,
                artist,
                cover_url: None,
                duration,
            };
            let _ = controls.set_metadata(metadata);
            let _ = controls.set_playback(souvlaki::MediaPlayback::Playing { progress: None });
        }
    }

    pub fn finished(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(controls) = self.controls.as_mut() {
            let _ = controls.set_playback(souvlaki::MediaPlayback::Stopped);
        }
    }

    pub fn set_paused(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(controls) = self.controls.as_mut() {
            let _ = controls.set_playback(souvlaki::MediaPlayback::Paused { progress: None });
        }
    }

    pub fn set_playing(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(controls) = self.controls.as_mut() {
            let _ = controls.set_playback(souvlaki::MediaPlayback::Playing { progress: None });
        }
    }

    pub fn take_events_rx(&mut self) -> Option<mpsc::Receiver<MediaControlAction>> {
        #[cfg(target_os = "linux")]
        {
            return self.events_rx.take();
        }

        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}

fn split_artist_title(display: &str) -> (Option<&str>, String) {
    if let Some((artist, title)) = display.split_once(" - ") {
        return (Some(artist), title.to_string());
    }
    (None, display.to_string())
}
