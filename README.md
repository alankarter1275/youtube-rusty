# YouTube Rusty (`youtube-rusty`)

A lightweight, modern desktop YouTube client for Linux (Wayland / SwayFX), built with **Rust**, **GTK4**, and **Libadwaita**.

## Features
- **Ultra Lightweight**: Zero Electron or Chromium runtime overhead (< 50MB RAM).
- **Fast Async Scraping**: Powered by `rustypipe` (native Rust YouTube API client) with sub-second queries.
- **Pixel-Perfect Thumbnails**: Crisp native image rendering with local disk caching (`~/.cache/youtube-rusty/`).
- **Modern Aesthetics**: Native Libadwaita dark theme with smooth responsive card grids.
- **Integrated MPV Streaming**: Clicking any video seamlessly launches your configured `mpv` player with hardware acceleration (VA-API), 360p default, and ModernZ OSD.

## Build and Run

### Prerequisites (Arch Linux)
```bash
sudo pacman -S rust cargo gtk4 libadwaita mpv yt-dlp
```

### Running
```bash
cd ~/Projects/youtube-rusty
cargo run --release
```

## Controls
- **Search**: Type in the top search bar and hit `Enter`.
- **Play Video**: Click on any video card to stream immediately in `mpv`.
- **Quality**: In `mpv`, click the `HQ` button or press `Shift + f` to switch video resolutions on the fly.
