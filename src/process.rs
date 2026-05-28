use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    fs::File,
    io::ErrorKind,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonCommand {
    Replace {
        inputs: Vec<String>,
    },
    ReplaceFrom {
        inputs: Vec<String>,
        start: usize,
    },
    UpdateQueue {
        inputs: Vec<String>,
        current: Option<usize>,
    },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonSnapshot {
    pub queue: Vec<String>,
    pub current: Option<usize>,
    /// Path of the track the daemon is actually playing. Unlike `current` (an index into
    /// `queue`) this still reflects playback when the playing track isn't part of the
    /// queue — e.g. a standalone library pick played over a different active playlist.
    #[serde(default)]
    pub playing: Option<String>,
}

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon.sock")
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon-state.json")
}

pub fn read_snapshot(data_dir: &Path) -> Result<Option<DaemonSnapshot>> {
    let path = state_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    let snapshot = serde_json::from_str(&content)?;
    Ok(Some(snapshot))
}

pub fn write_snapshot(data_dir: &Path, snapshot: &DaemonSnapshot) -> Result<()> {
    let path = state_path(data_dir);
    let content = serde_json::to_string(snapshot)?;
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));

    let mut tmp = File::create(&tmp_path)?;
    tmp.write_all(content.as_bytes())?;
    tmp.sync_all()?;
    fs::rename(&tmp_path, &path)?;

    if let Ok(dir) = File::open(data_dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub fn send_command(data_dir: &Path, command: &DaemonCommand) -> Result<()> {
    send_command_internal(data_dir, command).map_err(map_send_command_error)
}

enum SendCommandError {
    Transport(String),
    Daemon(String),
}

fn send_command_internal(
    data_dir: &Path,
    command: &DaemonCommand,
) -> std::result::Result<(), SendCommandError> {
    let mut stream = UnixStream::connect(socket_path(data_dir)).map_err(|error| {
        if matches!(
            error.kind(),
            ErrorKind::NotFound
                | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::AddrNotAvailable
        ) {
            SendCommandError::Transport("daemon is not running".to_string())
        } else {
            SendCommandError::Transport(format!("daemon transport error: {error}"))
        }
    })?;
    let payload = serde_json::to_string(command).map_err(|error| {
        SendCommandError::Daemon(format!("failed to serialize command: {error}"))
    })?;
    stream.write_all(payload.as_bytes()).map_err(|error| {
        SendCommandError::Transport(format!("failed to write command: {error}"))
    })?;
    stream.write_all(b"\n").map_err(|error| {
        SendCommandError::Transport(format!("failed to write command terminator: {error}"))
    })?;

    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        SendCommandError::Transport(format!("failed to read daemon response: {error}"))
    })?;
    if response.trim() == "ok" {
        Ok(())
    } else {
        Err(SendCommandError::Daemon(response.trim().to_string()))
    }
}

pub fn ensure_daemon_and_send_replace(data_dir: &Path, inputs: &[String]) -> Result<bool> {
    ensure_daemon_and_send_command(
        data_dir,
        &DaemonCommand::Replace {
            inputs: inputs.to_vec(),
        },
    )
}

pub fn ensure_daemon_and_send_command(data_dir: &Path, command: &DaemonCommand) -> Result<bool> {
    match send_command_internal(data_dir, command) {
        Ok(()) => return Ok(false),
        Err(SendCommandError::Daemon(message)) => return Err(anyhow::anyhow!(message)),
        Err(SendCommandError::Transport(_)) => {}
    }

    spawn_daemon(data_dir)?;
    wait_for_socket(data_dir)?;
    send_command_internal(data_dir, command).map_err(map_send_command_error)?;
    Ok(true)
}

fn map_send_command_error(error: SendCommandError) -> anyhow::Error {
    match error {
        SendCommandError::Transport(message) | SendCommandError::Daemon(message) => {
            anyhow::anyhow!(message)
        }
    }
}

pub fn wait_for_current_index(data_dir: &Path, expected: usize, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        match read_snapshot(data_dir) {
            Ok(Some(snapshot)) => {
                if snapshot.current == Some(expected) {
                    return Ok(());
                }
            }
            Ok(None) => {}
            Err(_) => {}
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("daemon did not report active playback in time")
}

pub fn stop_background_players(data_dir: &Path) -> Result<usize> {
    if send_command(data_dir, &DaemonCommand::Stop).is_ok() {
        return Ok(1);
    }
    Ok(0)
}

fn spawn_daemon(data_dir: &Path) -> Result<()> {
    fs::create_dir_all(data_dir)?;
    let socket = socket_path(data_dir);
    if socket.exists() {
        match UnixStream::connect(&socket) {
            Ok(_) => anyhow::bail!("daemon is already running"),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound
                        | ErrorKind::ConnectionRefused
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::AddrNotAvailable
                ) =>
            {
                let _ = fs::remove_file(&socket);
            }
            Err(error) => anyhow::bail!("cannot safely replace daemon socket: {error}"),
        }
    }
    let exe = std::env::current_exe()?;

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("setsid");
        command.arg(&exe);
        command
    };

    #[cfg(not(target_os = "linux"))]
    let mut command = Command::new(&exe);

    // macOS has no `setsid` binary, so detach the child into its own session from inside
    // the forked process instead, mirroring what `setsid orpheus` does on Linux. Without
    // this the daemon shares the launcher's process group and dies on terminal hangup.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                // `setsid(2)` is async-signal-safe, so it is safe to call here.
                if setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    command
        .arg("play-internal")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn setsid() -> i32;
}

fn wait_for_socket(data_dir: &Path) -> Result<()> {
    let socket = socket_path(data_dir);
    for _ in 0..80 {
        if socket.exists() && UnixStream::connect(&socket).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(40));
    }
    anyhow::bail!("daemon failed to start")
}
