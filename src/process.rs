use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonCommand {
    Replace { inputs: Vec<String> },
    ReplaceFrom { inputs: Vec<String>, start: usize },
    UpdateQueue { inputs: Vec<String>, current: Option<usize> },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonSnapshot {
    pub queue: Vec<String>,
    pub current: Option<usize>,
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
    fs::write(path, content)?;
    Ok(())
}

pub fn send_command(data_dir: &Path, command: &DaemonCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path(data_dir)).context("daemon is not running")?;
    let payload = serde_json::to_string(command)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    if response.trim() == "ok" {
        Ok(())
    } else {
        anyhow::bail!(response.trim().to_string())
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
    if send_command(data_dir, command).is_ok() {
        return Ok(false);
    }

    spawn_daemon(data_dir)?;
    wait_for_socket(data_dir)?;
    send_command(data_dir, command)?;
    Ok(true)
}

pub fn wait_for_current_index(data_dir: &Path, expected: usize, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(snapshot) = read_snapshot(data_dir)? {
            if snapshot.current == Some(expected) {
                return Ok(());
            }
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
        let _ = fs::remove_file(&socket);
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

    command
        .arg("play-internal")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
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
