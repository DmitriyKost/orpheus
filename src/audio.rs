use anyhow::{Context, Result};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::{fs::File, path::Path, time::Duration};

pub struct NativePlayer {
    _sink: MixerDeviceSink,
    player: Player,
}

impl NativePlayer {
    pub fn new() -> Result<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .context("failed to open default audio output")?;
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
        })
    }

    pub fn play(&mut self, path: &Path) -> Result<()> {
        self.player.stop();
        self.append(path)?;
        self.player.play();
        Ok(())
    }

    pub fn append(&self, path: &Path) -> Result<()> {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let source = Decoder::try_from(file)
            .with_context(|| format!("failed to decode {}", path.display()))?;
        self.player.append(source);
        Ok(())
    }

    pub fn toggle_pause(&self) {
        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn resume(&self) {
        self.player.play();
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    pub fn is_empty(&self) -> bool {
        self.player.empty()
    }

    pub fn sleep_until_end(&self) {
        self.player.sleep_until_end();
    }
}

pub fn duration(path: &Path) -> Option<Duration> {
    let file = File::open(path).ok()?;
    Decoder::try_from(file).ok()?.total_duration()
}
