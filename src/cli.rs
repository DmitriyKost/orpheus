use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "orpheus",
    version,
    about = "Small native terminal music player"
)]
pub struct Cli {
    /// Override the configured music directory.
    #[arg(long, global = true)]
    pub music_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the interactive TUI player.
    Tui,
    /// Print all discovered audio files.
    Scan,
    /// Play a file, directory, playlist name, or the whole library. Blocks until playback finishes.
    Play {
        /// Start playback in a detached process and return immediately.
        #[arg(long)]
        background: bool,
        input: Option<String>,
    },
    /// Stop all background Orpheus playback processes.
    Stop,
    /// Generate shell completion script.
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
    #[cfg(target_os = "macos")]
    /// Show current track in macOS menu bar.
    MenuBar,
    #[cfg(target_os = "macos")]
    #[command(hide = true)]
    MenuBarInternal,
    #[command(hide = true)]
    PlayInternal,
    /// Manage m3u playlists stored by Orpheus.
    Playlist {
        #[command(subcommand)]
        command: PlaylistCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
}

#[derive(Debug, Subcommand)]
pub enum PlaylistCommand {
    /// List playlists.
    List,
    /// Show paths in a playlist.
    Show { name: String },
    /// Create an empty playlist.
    Create { name: String },
    /// Delete a playlist.
    Delete { name: String },
    /// Append files to a playlist.
    Add { name: String, files: Vec<PathBuf> },
}
