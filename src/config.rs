use anyhow::{Context, Result};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Config {
    pub music_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Config {
    pub fn load(music_dir_override: Option<PathBuf>) -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?;
        let config_root = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let data_root = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));

        let config_dir = config_root.join("orpheus");
        let data_dir = data_root.join("orpheus");
        fs::create_dir_all(&config_dir)?;
        fs::create_dir_all(&data_dir)?;

        let config_path = config_dir.join("config.conf");
        if !config_path.exists() {
            fs::write(
                &config_path,
                "# Orpheus configuration\n# music_dir=$HOME/Music\n",
            )?;
        }

        let configured_music_dir =
            read_music_dir(&config_path, &home)?.unwrap_or_else(|| home.join("Music"));
        let music_dir = music_dir_override.unwrap_or(configured_music_dir);

        Ok(Self {
            music_dir,
            data_dir,
        })
    }
}

fn read_music_dir(path: &Path, home: &Path) -> Result<Option<PathBuf>> {
    let content = fs::read_to_string(path)?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(("music_dir", value)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
        {
            return Ok(Some(expand_home(value, home)));
        }
    }
    Ok(None)
}

fn expand_home(value: &str, home: &Path) -> PathBuf {
    if value == "$HOME" {
        return home.to_path_buf();
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(value)
}
