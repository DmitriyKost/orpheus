use std::path::PathBuf;

use anyhow::Result;

use crate::{
    library::{Library, Track},
    playlist::PlaylistStore,
};

pub fn resolve_inputs(
    library: &Library,
    playlists: &PlaylistStore,
    inputs: &[String],
) -> Result<Vec<Track>> {
    if inputs.is_empty() {
        return library.scan();
    }

    let mut tracks = Vec::new();
    for input in inputs {
        tracks.extend(resolve_one_input(playlists, input)?);
    }
    Ok(tracks)
}

fn resolve_one_input(playlists: &PlaylistStore, input: &str) -> Result<Vec<Track>> {
    let input_path = PathBuf::from(input);
    if input_path.exists() {
        if input_path.is_dir() {
            return Library::new(input_path).scan();
        }
        return Ok(vec![Track::from_path(input_path)]);
    }

    let playlist_tracks = playlists.read(input)?;
    Ok(playlist_tracks.into_iter().map(Track::from_path).collect())
}
