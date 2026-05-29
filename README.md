# Orpheus

Orpheus is a terminal music player with a background playback daemon, vim-like keybindings, playlist support, and MPRIS/media-key integration on Linux.

## Features

- Native playback with `rodio`
- Interactive TUI (`ratatui` + `crossterm`)
- Background daemon controlled from TUI and CLI
- M3U playlist management
- MPRIS metadata + media keys (`play/pause/next/previous`) on Linux
- Vim-like navigation and command mode in TUI (`:q`, `ZZ`)

## Build

```sh
cargo build --release
```

## Run

Launch TUI:

```sh
orpheus
# or
orpheus tui
```

Scan library:

```sh
orpheus scan
```

Foreground playback:

```sh
orpheus play ~/Music/song.flac
orpheus play ~/Music/Albums
orpheus play my-playlist
orpheus play
```

Background daemon playback (start or update current daemon):

```sh
orpheus play --background ~/Music/song.flac
orpheus play --background my-playlist
```

Stop daemon playback:

```sh
orpheus stop
```

## TUI Workflow

- Use `h/j/k/l`, `gg`, `G`, `Ctrl-d`, `Ctrl-u` to navigate
- Press `p` in the left pane to toggle **Library** / **Playlists** view
- Press `Enter` on a **library track** to play that single track immediately (does not replace queue)
- Press `Enter` on a **playlist** to load it as the active queue and start playback from item 1
- Press `a` to append selected library track to the active playlist-backed queue
- Press `dd` to remove selected queue entry
- Press `dj` / `dk` to delete selected+next / selected+previous queue entries
- Press `J` / `K` to move selected queue entry down/up
- Press `/` to search in active pane (Library or Queue)
- Press `Esc` to clear active search filter
- Press `S` to save current queue to the active playlist
- Press `v` to edit the active playlist in `$EDITOR` (defaults to `nvim`) and return to TUI
- In Playlists view: `c` opens command mode with `plnew `, `dd` deletes selected playlist
- Use `:q` or `ZZ` to quit

## TUI Keys

```text
h/j/k/l          pane + movement
gg, G            first / last
Ctrl-d / Ctrl-u  page down / page up
Tab              switch Library / Queue pane
Enter            library: play single track; playlists: activate playlist; queue: play selected item
p                toggle left pane: Library / Playlists
a                append selected library track to active queue
dd               queue: remove selected item; playlists: delete selected playlist
dj / dk          queue: delete selected+next / selected+previous
J / K            move selected queue item down / up
/                search active pane
Esc              clear search filter
S                save queue to active playlist
v                edit active playlist in editor, then return
c                in Playlists view: open command mode with `plnew `
:                command mode
:plnew <name>    create playlist
:q               quit
ZZ               quit
```

## Playerctl / Media Keys (Linux)

Check player:

```sh
playerctl -l
```

Read metadata:

```sh
playerctl --player=orpheus metadata
```

Control playback:

```sh
playerctl --player=orpheus play-pause
playerctl --player=orpheus next
playerctl --player=orpheus previous
```

## Playlists

```sh
orpheus playlist list
orpheus playlist create favorites
orpheus playlist add favorites ~/Music/a.flac ~/Music/b.mp3
orpheus playlist show favorites
orpheus playlist delete favorites
```

TUI queue model:

- The queue is backed by the currently active playlist (default: `tui-queue`).
- Queue edits in TUI (`a`, `dd`, `dj`, `dk`, `J`, `K`, `S`, editor save) update playlist content on disk.
- The queue pane title shows the active playlist name.

## Configuration

Config file:

```text
$XDG_CONFIG_HOME/orpheus/config.conf
```

Supported option:

```text
music_dir=$HOME/Music
```

Data directory:

```text
$XDG_DATA_HOME/orpheus
```

Playlists directory:

```text
$XDG_DATA_HOME/orpheus/playlists
```

## Notes

- The daemon state is cached under the data directory and restored in TUI.
- If `playerctl` is used with multiple players running, use `--player=orpheus` explicitly.
