use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use std::io::{self, Write};

pub fn run_fzf(files: &[PathBuf], multi: bool) -> io::Result<Vec<PathBuf>> {
    let mut cmd = Command::new("fzf");

    if multi {
        cmd.arg("-m");
    }

    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;

    {
        let mut stdin = child.stdin.take().expect("Failed to open fzf stdin");
        for f in files {
            writeln!(stdin, "{}", f.display())?;
        }
        drop(stdin);
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let selected = String::from_utf8_lossy(&output.stdout);

    Ok(selected
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .collect())
}
