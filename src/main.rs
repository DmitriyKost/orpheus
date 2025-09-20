mod config;
mod mpv;
mod playlist;
mod ui;

use mpv::*;
use playlist::{edit_playlist, list_playlists, scan_music};
use std::{env, path::PathBuf};
use ui::run_fzf;

use crate::playlist::{create_playlist, delete_playlists, jump};

#[derive(Debug)]
enum Command {
    List,
    Create { name: String },
    Edit,
    Delete,
    Play,
    Append,
    Reload,
    Jump,
    Help,
}

impl Command {
    fn all() -> &'static [&'static str] {
        &[
            "list", "create", "edit", "delete", "play", "append", "reload", "jump", "help",
        ]
    }

    fn parse(args: &[String]) -> Option<Command> {
        match args.get(0).map(|s| s.as_str()) {
            Some("list") => Some(Command::List),
            Some("create") => args
                .get(1)
                .map(|name| Command::Create { name: name.clone() }),
            Some("edit") => Some(Command::Edit),
            Some("delete") => Some(Command::Delete),
            Some("play") => Some(Command::Play),
            Some("append") => Some(Command::Append),
            Some("reload") => Some(Command::Reload),
            Some("jump") => Some(Command::Jump),
            Some("help") => Some(Command::Help),
            _ => None,
        }
    }
}

fn print_usage() {
    println!(
        "Usage: orpheus <command> [args]\n\n\
        Commands:\n\
        \tlist\t\t\tList all playlists\n\
        \tcreate <name>\t\tCreate a new playlist\n\
        \tedit\t\t\tEdit a playlist\n\
        \tdelete\t\t\tDelete playlists\n\
        \tplay\t\t\tSelect and play a track or playlist\n\
        \tappend\t\t\tAppend tracks to queue\n\
        \treload\t\t\tReload mpv with updated configuration\n\
        \tjump\t\t\tJumps to a track in current queue\n\
        \thelp\t\t\tPrints this cheatsheet\n"
    );
}

fn print_completions(words: &[String]) -> std::io::Result<()> {
    match words.len() {
        0 | 1 => {
            for cmd in Command::all() {
                println!("{}", cmd);
            }
        }
        _ => {}
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let config = config::Config::load()?;
    config::CONFIG
        .set(config)
        .expect("Config already initialized");

    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    if args[0] == "--complete" {
        print_completions(&args[1..])?;
        return Ok(());
    }

    mpv::spawn()?;

    let command = match Command::parse(&args) {
        Some(cmd) => cmd,
        None => {
            eprintln!("Unknown command or missing arguments.");
            print_usage();
            return Ok(());
        }
    };

    match command {
        Command::List => {
            let playlists = list_playlists()?;
            println!("Playlists:");
            for p in playlists {
                println!("{}", p.file_name().unwrap().to_string_lossy());
            }
        }

        Command::Create { name } => create_playlist(&name)?,

        Command::Edit => edit_playlist()?,

        Command::Delete => delete_playlists()?,

        Command::Play => {
            let options = vec!["playlist", "single file"];
            let choice = run_fzf(
                &options.iter().map(|s| PathBuf::from(s)).collect::<Vec<_>>(),
                false,
            )?;
            if choice.is_empty() {
                println!("No choice selected.");
                return Ok(());
            }

            match choice[0].to_string_lossy().as_ref() {
                "playlist" => {
                    let playlists = list_playlists()?;
                    let selected = run_fzf(&playlists, false)?;
                    if selected.is_empty() {
                        println!("No playlist selected.");
                        return Ok(());
                    }
                    let playlist_path = &selected[0];
                    send_command(MpvCommand::LoadPlaylist {
                        path: playlist_path.to_string_lossy().into(),
                    })?;
                }
                "single file" => {
                    let files = scan_music()?;
                    let selected = run_fzf(&files, false)?;
                    if selected.is_empty() {
                        println!("No file selected.");
                        return Ok(());
                    }
                    for file in &selected {
                        send_command(MpvCommand::PlayFile {
                            path: file.to_string_lossy().into(),
                        })?;
                    }
                }
                _ => {}
            }
        }

        Command::Append => {
            let files = scan_music()?;
            let selected = run_fzf(&files, true)?;
            if selected.is_empty() {
                println!("No file selected.");
                return Ok(());
            }
            for file in &selected {
                send_command(MpvCommand::AppendFile {
                    path: file.to_string_lossy().into(),
                })?;
            }
        }

        Command::Reload => {
            send_command(MpvCommand::Quit)?;
            println!("Stopped existing mpv instance...");
            std::thread::sleep(std::time::Duration::from_millis(300));
            mpv::spawn()?;
            println!("Started new mpv with updated configuration.");
        }

        Command::Jump => {
            let idx = jump()?;
            send_command(MpvCommand::JumpTo { index: idx })?
        }

        Command::Help => print_usage(),
    }

    Ok(())
}
