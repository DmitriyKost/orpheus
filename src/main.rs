mod config;
mod mpv;
mod playlist;
mod ui;

use mpv::*;
use playlist::{edit_playlist, list_playlists, scan_music};
use std::{env, path::PathBuf};
use ui::run_fzf;

use crate::playlist::{create_playlist, delete_playlists};

fn print_usage() {
    println!(
        "Usage: orpheus <command> [args]\n\n\
        Commands:\n\
        \tlist\t\t\tList all playlists\n\
        \tcreate <name>\t\tCreate a new playlist\n\
        \tedit\t\t\tEdit a playlist\n\
        \tdelete\t\t\tDelete playlists\n\
        \tplay\t\t\tSelect and play a track or playlist\n\
        \tappend\t\t\tAppend tracks to queue
        \thelp\t\t\tPrints this cheatsheet"
    );
}

fn main() -> std::io::Result<()> {
    let config = config::Config::load()?;
    config::CONFIG
        .set(config)
        .expect("Config already initialized");

    mpv::spawn()?;

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "list" => {
            let playlists = list_playlists()?;
            println!("Playlists:");
            for p in playlists {
                println!("{}", p.file_name().unwrap().to_string_lossy());
            }
        }

        "create" => {
            let name = args.get(2).expect("Please provide a playlist name");
            create_playlist(&name)?
        }

        "edit" => edit_playlist()?,

        "delete" => delete_playlists()?,

        "play" => {
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
                    let selected = run_fzf(&files, true)?;
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

        "append" => {
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

        "reload" => {
            send_command(MpvCommand::Quit)?;
            println!("Stopped existing mpv instance...");
            std::thread::sleep(std::time::Duration::from_millis(300));
            mpv::spawn()?;
            println!("Started new mpv with updated configuration.");
        }

        "help" => {
            print_usage();
        }

        _ => {
            eprintln!("Unknown command: {}", args[1]);
            print_usage();
        }
    }

    Ok(())
}
