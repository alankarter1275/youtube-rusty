# YouTube Rusty (`youtube-rusty`)

A fast, lightweight desktop YouTube app for Linux, built with **Rust**, **GTK4**, **Libadwaita**, **RustyPipe**, and **MPV** — with zero Python or `yt-dlp` dependency.

## Features
- **Instant Direct Playback (No `yt-dlp`)**: Resolves direct video and audio streams natively in Rust (`--ytdl=no`), preferring lightweight codecs (`H.264` / `VP9` over `AV1`) so videos open quickly even on modest hardware.
- **Desktop Mouse Controls**:
  - **Single-click** any card to select/highlight it (`Enter` plays the selected card).
  - **Double-click** any card to play immediately in `mpv`.
  - **Right-click** any card to open the Video Details popup (quality selector, Queue, Playlist +, Watch later, Copy link, Download).
- **Complete Search Experience**:
  - Live YouTube search suggestions while typing + saved Search History (remove individual items or clear all).
  - Filter by **All**, **Videos**, **Channels**, or **Playlists**, plus upload date, duration, and sort order.
  - Paste any YouTube video, channel, or playlist link directly into the search bar to open it right away.
- **Playlists & Subscriptions**:
  - Create playlists, save your current playing queue, save YouTube playlists, rename/delete playlists, reorder tracks, and **Play all** or **Shuffle** from the left sidebar.
  - Subscribe to channels from search results or channel pages and browse their latest uploads in **Subscriptions**.
- **Bandwidth-Friendly Infinite Scroll**: Clean hidden scrollbars with automatic loading only when scrolling near the bottom of a list.
- **Pure-Rust Downloads**: Save videos and audio directly to your `Downloads` folder (`ffmpeg` optional for lossless HD remuxing).
- **Customizable Look**: Warm Kalam dark theme by default, with Light/System mode and Terracotta, Forest, Plum, and Teal accent colors in Preferences.

## Install Requirements

### Arch Linux
```bash
sudo pacman -S rust cargo gtk4 libadwaita mpv
```
*(Optional: install `ffmpeg` if you want 720p/1080p downloads combined automatically.)*

## How to Run
```bash
cargo run --release
```

## Controls & Shortcuts
- **Single-click card**: Highlight / select
- **Double-click** or **`Enter`**: Play in MPV
- **Right-click card**: Open video details & options
- **`/`** or **`Ctrl + F`**: Jump to the search bar
- **`Up` / `Down`**: Move through search history & live suggestions
- **`Esc`**: Close dropdowns or leave the search bar
- **`Alt + Left`**: Go back
- **`Space`**: Pause or resume when the player card is selected

## Performance diagnostics

Optional terminal measurements for troubleshooting are described in [docs/diagnostics.md](docs/diagnostics.md).
