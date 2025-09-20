use std::{
    fmt,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::Command,
    thread::sleep,
    time::Duration,
};

use crate::config::CONFIG;

pub enum MpvCommand {
    /// Load a new playlist file (replace current playlist)
    LoadPlaylist { path: String },
    /// Append a track to the playlist
    AppendFile { path: String },
    /// Play a single file (replace current playlist)
    PlayFile { path: String },
    /// Quit mpv gracefully
    Quit,
    /// Get the current queue
    GetQueue,
    /// Jump to a track in the playlist by index
    JumpTo { index: usize },
}

impl fmt::Display for MpvCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MpvCommand::LoadPlaylist { path } => {
                write!(f, r#""loadlist", "{}", "replace""#, path)
            }
            MpvCommand::AppendFile { path } => {
                write!(f, r#""loadfile", "{}", "append-play""#, path)
            }
            MpvCommand::PlayFile { path } => {
                write!(f, r#""loadfile", "{}", "replace""#, path)
            }
            MpvCommand::Quit => write!(f, r#""quit""#),
            MpvCommand::GetQueue => write!(f, r#""get_property", "playlist""#),
            MpvCommand::JumpTo { index } => {
                write!(f, r#""set_property", "playlist-pos", {}"#, index)
            }
        }
    }
}

fn mpv_json_command(cmd: MpvCommand) -> String {
    format!(r#"{{ "command": [{}] }}"#, cmd) + "\n"
}

pub fn is_running() -> bool {
    let config = CONFIG.get().expect("config not initialized");

    let socket_path = &config.socket_path;

    if !Path::new(socket_path).exists() {
        return false;
    }

    UnixStream::connect(socket_path).is_ok()
}

pub fn spawn() -> io::Result<()> {
    let config = CONFIG.get().expect("config not initialized");
    if Path::new(&config.socket_path).exists() {
        if UnixStream::connect(&config.socket_path).is_ok() {
            return Ok(());
        } else {
            std::fs::remove_file(&config.socket_path)?;
        }
    }

    let mut cmd = Command::new("mpv");

    if let Some(back) = &config.mpv_audio_backend {
        cmd.arg("--ao=".to_owned() + back);
    }

    if let Some(ref path) = config.mpris_plugin_path {
        if path.exists() {
            cmd.arg(format!(
                "--script={}",
                path.to_str().expect("invalid mpris_plugin_path")
            ));
        }
    }

    cmd.arg("--idle=yes")
        .arg("--no-video")
        .arg("--af=dynaudnorm=f=75:g=25:p=0.55")
        .arg("--force-window=no")
        .arg("--really-quiet")
        .arg("--no-terminal")
        .arg(format!(
            "--input-ipc-server={}",
            config.socket_path.to_str().expect("invalid socket_path")
        ));

    cmd.spawn()?;

    for _ in 0..10 {
        if Path::new(&config.socket_path).exists() {
            break;
        }
        sleep(Duration::from_millis(200));
    }
    if !Path::new(&config.socket_path).exists() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Failed to create mpv socket",
        ));
    }

    Ok(())
}

pub fn send_command(cmd: MpvCommand) -> io::Result<()> {
    let config = CONFIG.get().expect("config not initialized");
    let mut stream = UnixStream::connect(&config.socket_path)?;

    stream.write_all(mpv_json_command(cmd).as_bytes())?;
    stream.flush()?;

    Ok(())
}

pub fn get_queue() -> io::Result<Vec<String>> {
    let config = CONFIG.get().expect("config not initialized");
    let mut stream = UnixStream::connect(&config.socket_path)?;

    let cmd = mpv_json_command(MpvCommand::GetQueue);
    stream.write_all(cmd.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        response.push_str(&line);
        if line.contains(r#""error":"success""#) {
            break;
        }
    }

    let mut queue = Vec::new();
    let mut start = 0;
    while let Some(pos) = response[start..].find(r#""filename":"#) {
        let pos = start + pos;
        let rest = &response[pos + 12..];
        if let Some(end) = rest.find('"') {
            let filename = rest[..end].to_string();
            queue.push(filename);
            start = pos + 12 + end;
        } else {
            break;
        }
    }

    Ok(queue)
}
