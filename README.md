# Orpheus

**Orpheus** is a lightweight command-line music manager and built on top of `mpv`, with playlist management and fuzzy selection via `fzf`. It supports both individual tracks and playlists, with optional MPRIS integration for desktop media controls.

---

## Features

* List, create, edit, and delete playlists
* Play single tracks or entire playlists
* Append tracks to the current queue
* Reload mpv with updated configuration (`reload` command)
* Fuzzy search for tracks and playlists using `fzf`
* Optional MPRIS plugin support

---

## Dependencies

Before using Orpheus, ensure you have the following installed:

* **[mpv](https://mpv.io/)** – media player
* **[fzf](https://github.com/junegunn/fzf)** – fuzzy finder
* **mpv MPRIS plugin** (optional) – for desktop media integration

  * Default path: `/usr/lib/mpv-mpris/mpris.so`

---

## Installation

Clone the repository and build with Rust:

```bash
git clone https://github.com/DmitriyKost/orpheus.git
cd orpheus
cargo build --release
```

The compiled binary will be available at:

```bash
target/release/orpheus
```

---

## Configuration

Orpheus uses a configuration file at:

```
~/.config/orpheus/config.conf
```

Example config:

```text
# Orpheus configuration file
# socket_path=/tmp/mpv-socket
# mpris_plugin_path=/usr/lib/mpv-mpris/mpris.so
# music_dir=$HOME/Music
```

* `socket_path` – mpv IPC socket path
* `mpris_plugin_path` – path to mpv MPRIS plugin
* `music_dir` – default music directory

**Note:** The config file is auto-created on first run if missing. Environment variables like `$HOME` are expanded automatically.

---

## Usage

```
Usage: orpheus <command> [args]

Commands:
    list            List all playlists
    create <name>   Create a new playlist
    edit            Edit a playlist (delete or append tracks)
    delete          Delete playlists
    play            Select and play a track or playlist
    append          Append tracks to queue
    reload          Reload mpv with updated configuration
    jump            Jumps to a track in current queue
    help            Prints this cheatsheet
```

---

## Notes

* Uses `fzf` for interactive selection.
* Tracks starting with `#` in playlists are ignored as comments.
* MPRIS integration is optional; only loaded if the plugin path exists.
* The `reload` command gracefully stops the current mpv instance and restarts it with the latest configuration.
* **Playlists are stored under your XDG data directory:**

```
$XDG_DATA_HOME/orpheus/*.m3u
```

* If `$XDG_DATA_HOME` is not set, defaults to:

```
$HOME/.local/share/orpheus/*.m3u
```
