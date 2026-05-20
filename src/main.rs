mod audio;
mod cli;
mod config;
mod daemon;
mod library;
mod media;
mod playlist;
mod process;
mod tui;

use anyhow::Result;
use clap::Parser;
use crate::{
    audio::{duration, NativePlayer},
    cli::{Cli, Command, PlaylistCommand},
    config::Config,
    library::{Library, Track},
    media::MediaSession,
    playlist::PlaylistStore,
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.music_dir.clone())?;
    let library = Library::new(config.music_dir.clone());
    let playlists = PlaylistStore::new(config.data_dir.clone())?;

    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => {
            let tracks = library.scan()?;
            tui::run(config, tracks, playlists)?;
        }
        Command::Scan => {
            for track in library.scan()? {
                println!("{}", track.path.display());
            }
        }
        Command::Play { input, background } => {
            let inputs = input.into_iter().collect::<Vec<_>>();
            if background {
                let started = process::ensure_daemon_and_send_replace(&config.data_dir, &inputs)?;
                if started {
                    println!("started background playback process");
                } else {
                    println!("updated running background playback");
                }
            } else {
                let tracks = resolve_inputs(&library, &playlists, &inputs)?;
                play_tracks(tracks)?;
            }
        }
        Command::Stop => {
            let killed = process::stop_background_players(&config.data_dir)?;
            println!("stopped {killed} background process(es)");
        }
        Command::PlayInternal { .. } => {
            daemon::run(config, library, playlists)?;
        }
        Command::Playlist { command } => match command {
            PlaylistCommand::List => {
                for playlist in playlists.list()? {
                    println!("{}", playlist.name);
                }
            }
            PlaylistCommand::Show { name } => {
                for track in playlists.read(&name)? {
                    println!("{}", track.display());
                }
            }
            PlaylistCommand::Create { name } => {
                playlists.create(&name)?;
                println!("created playlist {name}");
            }
            PlaylistCommand::Delete { name } => {
                playlists.delete(&name)?;
                println!("deleted playlist {name}");
            }
            PlaylistCommand::Add { name, files } => {
                playlists.append(&name, &files)?;
                println!("added {} file(s) to {name}", files.len());
            }
        },
    }

    Ok(())
}

fn play_tracks(tracks: Vec<Track>) -> Result<()> {
    if tracks.is_empty() {
        anyhow::bail!("nothing to play");
    }

    let mut player = NativePlayer::new()?;
    let mut media = MediaSession::new();

    for track in tracks {
        println!("Playing {}", track.display_name());
        media.track_started(&track, duration(&track.path));
        player.play(&track.path)?;
        player.sleep_until_end();
    }

    media.finished();
    Ok(())
}

fn resolve_inputs(library: &Library, playlists: &PlaylistStore, inputs: &[String]) -> Result<Vec<library::Track>> {
    if inputs.is_empty() {
        return Ok(library.scan()?);
    }

    let mut tracks = Vec::new();
    for input in inputs {
        tracks.extend(resolve_one_input(playlists, input)?);
    }
    Ok(tracks)
}

fn resolve_one_input(playlists: &PlaylistStore, input: &str) -> Result<Vec<library::Track>> {
    let input_path = std::path::PathBuf::from(input);
    if input_path.exists() {
        if input_path.is_dir() {
            return Ok(Library::new(input_path).scan()?);
        }
        return Ok(vec![library::Track::from_path(input_path)]);
    }

    let playlist_tracks = playlists.read(input)?;
    Ok(playlist_tracks.into_iter().map(library::Track::from_path).collect())
}
