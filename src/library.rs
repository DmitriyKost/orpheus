use anyhow::Result;
use std::{ffi::OsStr, path::PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
}

impl Track {
    pub fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn display_name(&self) -> String {
        self.path
            .file_stem()
            .or_else(|| self.path.file_name())
            .and_then(OsStr::to_str)
            .map(str::to_string)
            .unwrap_or_else(|| self.path.to_string_lossy().to_string())
    }

    pub fn short_path(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn scan(&self) -> Result<Vec<Track>> {
        let mut tracks = Vec::new();
        if !self.root.exists() {
            return Ok(tracks);
        }

        for entry in WalkDir::new(&self.root)
            .follow_links(true)
            .into_iter()
            .filter_entry(|entry| !is_hidden(entry.file_name()))
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && is_audio(path.extension().and_then(OsStr::to_str)) {
                tracks.push(Track::from_path(path.to_path_buf()));
            }
        }
        tracks.sort_by_key(|track| track.short_path().to_lowercase());
        Ok(tracks)
    }
}

fn is_hidden(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with('.'))
}

fn is_audio(ext: Option<&str>) -> bool {
    matches!(
        ext.map(str::to_ascii_lowercase).as_deref(),
        Some("mp3" | "flac" | "ogg" | "oga" | "wav" | "m4a" | "aac")
    )
}
