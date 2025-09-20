use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::config::CONFIG;
use crate::mpv::get_queue;
use crate::ui::run_fzf;

fn get_orpheus_dir() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").expect("HOME env var not set");
            PathBuf::from(home).join(".local/share")
        });
    let orpheus_dir = data_dir.join("orpheus");
    fs::create_dir_all(&orpheus_dir).unwrap();
    orpheus_dir
}

pub fn list_playlists() -> io::Result<Vec<PathBuf>> {
    let orpheus_dir = get_orpheus_dir();
    let mut playlists = Vec::new();
    for entry in fs::read_dir(&orpheus_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |e| e == "m3u") {
            playlists.push(path);
        }
    }
    Ok(playlists)
}

pub fn create_playlist(name: &str) -> io::Result<()> {
    let files = scan_music()?;
    let selected = run_fzf(&files, true)?;
    let path = write_playlist(name, &selected)?;
    println!("Created playlist at {}", path.display());
    Ok(())
}

fn write_playlist(name: &str, files: &[PathBuf]) -> io::Result<PathBuf> {
    let playlist_path = get_orpheus_dir().join(format!("{}.m3u", name));
    let mut file = File::create(&playlist_path)?;

    writeln!(file, "#EXTM3U")?;
    for f in files {
        writeln!(file, "{}", f.display())?;
    }

    Ok(playlist_path)
}

pub fn edit_playlist() -> io::Result<()> {
    let playlists = list_playlists()?;
    if playlists.is_empty() {
        eprintln!("No playlists available.");
        return Ok(());
    }

    let selected_playlist = run_fzf(&playlists, false)?;
    if selected_playlist.is_empty() {
        return Ok(());
    }
    let playlist_path = &selected_playlist[0];
    let playlist_name = playlist_path.file_stem().unwrap().to_string_lossy();

    let actions = vec!["delete", "append"];
    let action_selected = run_fzf(
        &actions.iter().map(|s| PathBuf::from(s)).collect::<Vec<_>>(),
        false,
    )?;
    if action_selected.is_empty() {
        return Ok(());
    }

    let mut playlist_tracks: Vec<PathBuf> = {
        let file = File::open(playlist_path)?;
        BufReader::new(file)
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.starts_with('#'))
            .map(PathBuf::from)
            .collect()
    };

    match action_selected[0].to_string_lossy().as_ref() {
        "delete" => {
            let to_delete = run_fzf(&playlist_tracks, true)?;
            playlist_tracks.retain(|f| !to_delete.contains(f));
            println!("Deleted {} track(s).", to_delete.len());
        }

        "append" => {
            let music_files = scan_music()?;
            let to_append_candidates: Vec<_> = music_files
                .into_iter()
                .filter(|f| !playlist_tracks.contains(f))
                .collect();

            if to_append_candidates.is_empty() {
                println!("No new tracks available to append.");
            } else {
                let to_append = run_fzf(&to_append_candidates, true)?;
                playlist_tracks.extend(to_append);
                println!("Appended {} track(s).", playlist_tracks.len());
            }
        }

        _ => {}
    }

    write_playlist(&playlist_name, &playlist_tracks)?;
    Ok(())
}

pub fn delete_playlists() -> io::Result<()> {
    let playlists = list_playlists()?;
    if playlists.is_empty() {
        eprintln!("No playlists available to delete.");
        return Ok(());
    }

    let selected = run_fzf(&playlists, true)?;
    if selected.is_empty() {
        return Ok(());
    }

    for playlist_path in &selected {
        if let Err(e) = fs::remove_file(playlist_path) {
            eprintln!("Failed to delete {}: {}", playlist_path.display(), e);
        }
    }

    Ok(())
}

pub fn scan_music() -> io::Result<Vec<PathBuf>> {
    let config = CONFIG.get().expect("config not initialized");
    let music_dir = &config.music_dir;

    let mut files = Vec::new();
    for entry in std::fs::read_dir(music_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext.to_lowercase().as_str() {
                    "mp3" | "flac" | "ogg" | "wav" => files.push(path),
                    _ => {}
                }
            }
        }
    }
    Ok(files)
}

pub fn jump() -> io::Result<Option<usize>> {
    let queue = get_queue()?;

    if queue.is_empty() {
        return Err(io::Error::new(io::ErrorKind::Other, "queue is empty"));
    }

    let selected = run_fzf(
        &queue.iter().map(|s| PathBuf::from(s)).collect::<Vec<_>>(),
        false,
    )?;

    if selected.is_empty() {
        return Ok(None);
    }

    let chosen_filename = selected[0].to_string_lossy().to_string();
    if let Some(index) = queue.iter().position(|f| *f == chosen_filename) {
        Ok(Some(index))
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("failed to find the index of {}", chosen_filename),
        ))
    }
}
