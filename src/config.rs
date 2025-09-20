use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;

use std::sync::OnceLock;

pub static CONFIG: OnceLock<Config> = OnceLock::new();

#[derive(Debug)]
pub struct Config {
    pub socket_path: PathBuf,
    pub mpris_plugin_path: Option<PathBuf>,
    pub music_dir: PathBuf,
    pub mpv_audio_backend: Option<String>,
}

impl Config {
    pub fn load() -> io::Result<Self> {
        let home_dir = env::var("HOME")
            .map(PathBuf::from)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let default_socket = PathBuf::from("/tmp/mpv-socket");
        let default_music = home_dir.join("Music");

        let xdg_config = env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir.join(".config"));
        let config_dir = xdg_config.join("orpheus");
        let config_path = config_dir.join("config.conf");

        if !config_dir.exists() {
            fs::create_dir_all(&config_dir)?;
        }

        if !config_path.exists() {
            let mut file = File::create(&config_path)?;
            writeln!(file, "# Orpheus configuration file")?;
            writeln!(file, "# socket_path=/tmp/mpv-socket")?;
            writeln!(
                file,
                "# mpris_plugin_path=/usr/lib/mpv-mpris/mpris.so # Optional plugin - allows to use media keys"
            )?;
            writeln!(file, "# music_dir=$HOME/Music")?;
            writeln!(
                file,
                "# mpv_audio_backend=pulse # Optional arg for mpv --ao=[]"
            )?;
        }

        let mut config_map = HashMap::new();
        if let Ok(content) = fs::read_to_string(&config_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    config_map.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }

        let mpv_audio_backend = config_map.get("mpv_audio_backend").map(|v| v.clone());

        let socket_path = config_map
            .get("socket_path")
            .map(|v| expand_env_vars(v))
            .unwrap_or(default_socket);

        let music_dir = config_map
            .get("music_dir")
            .map(|v| expand_env_vars(v))
            .unwrap_or(default_music);

        let mpris_plugin_path = config_map
            .get("mpris_plugin_path")
            .map(|v| expand_env_vars(v))
            .and_then(|p| {
                let path = PathBuf::from(p);
                if path.exists() { Some(path) } else { None }
            });

        Ok(Self {
            socket_path,
            mpris_plugin_path,
            music_dir,
            mpv_audio_backend,
        })
    }
}

fn expand_env_vars(path: &str) -> PathBuf {
    let mut result = String::new();
    let mut chars = path.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            let mut var_name = String::new();
            if let Some(&next) = chars.peek() {
                if next == '{' {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        if ch == '}' {
                            chars.next();
                            break;
                        }
                        var_name.push(ch);
                        chars.next();
                    }
                } else {
                    while let Some(&ch) = chars.peek() {
                        if !ch.is_alphanumeric() && ch != '_' {
                            break;
                        }
                        var_name.push(ch);
                        chars.next();
                    }
                }
            }
            let value = env::var(&var_name).unwrap_or_default();
            result.push_str(&value);
        } else {
            result.push(c);
        }
    }

    PathBuf::from(result)
}
