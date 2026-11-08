# YouTube Rusty (`youtube-rusty`)

A fast, lightweight desktop YouTube app for Linux, built with **Rust**, **GTK4**, and **Libadwaita**.

## Features
- **Fast & Lightweight**: Uses very little memory compared to a web browser.
- **Home, Search & Categories**: Browse Trending, Gaming, Music, News, or search YouTube with upload date, length, and sort filters.
- **Channel Pages & Shorts**: View any channel's Videos and Shorts, search inside a channel, filter, sort, and subscribe locally.
- **Subscriptions & History**: Follow your favorite channels and keep track of watched videos on your computer (`~/.config/youtube-rusty/`).
- **Watch Later & Downloads**: Save videos to Watch Later or download videos and audio directly to your `Downloads` folder using `yt-dlp`.
- **MPV Player & Queue Card**: Streams videos in `mpv` (360p, 720p, 1080p, or Audio only) with a built-in floating queue card that supports pause/resume, next/previous, shuffle, remove, and drag-and-drop reordering.
- **Customizable Look**: Warm Kalam dark theme by default, with Light/System mode and Terracotta, Forest, Plum, and Teal accent colors in Preferences.

## Install Requirements

### Arch Linux
```bash
sudo pacman -S rust cargo gtk4 libadwaita mpv yt-dlp
```

## How to Run
```bash
cargo run --release
```

## Helpful Shortcuts
- **`/`** or **`Ctrl + F`**: Jump to the search bar
- **`Enter`**: Start search
- **`Esc`**: Close popups or leave the search bar
- **`Alt + Left`**: Go back from a Channel page
- **`Space`**: Pause or resume when the player card is selected
