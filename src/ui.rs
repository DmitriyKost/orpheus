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
        let stdin = child.stdin.as_mut().unwrap();
        for f in files {
            writeln!(stdin, "{}", f.display())?;
        }
    }

    let output = child.wait_with_output()?;
    let selected = String::from_utf8_lossy(&output.stdout);

    Ok(selected
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .collect())
}
