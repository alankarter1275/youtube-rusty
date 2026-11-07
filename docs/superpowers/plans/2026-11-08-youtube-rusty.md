# youtube-rusty Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a lightweight, responsive YouTube desktop client using Rust, GTK4, and Libadwaita that renders video cards with thumbnails and launches playback in system `mpv`.

**Architecture:** A Libadwaita application (`AdwApplicationWindow`) with an adaptive sidebar and header search bar. A background Tokio thread handles asynchronous queries via `rustypipe` and network image fetching, communicating with the GTK main loop via `glib::MainContext::channel`. Double-clicking video cards spawns external `mpv` with the video URL and configured 360p/ModernZ profile.

**Tech Stack:** Rust (2021), GTK4 (`gtk4`), Libadwaita (`libadwaita`), Tokio (`tokio`), RustyPipe (`rustypipe`), Reqwest (`reqwest`), Serde (`serde`, `serde_json`).

**Spec:** [design_youtube_rusty.md](file:///home/kunalsingh/.gemini/antigravity-cli/brain/63ac4621-0571-4c95-af11-c9324b975737/design_youtube_rusty.md)

## Global Constraints
- Target platform: Linux / Wayland (SwayFX) on Intel Gemini Lake UHD 605.
- Minimal CPU overhead: No local Python yt-dlp execution for searching; use native Rust async scraping (`rustypipe`).
- External player: Spawns `/usr/bin/mpv` asynchronously without blocking GTK UI events.
- Responsive UI: Image decoding done on background threads; only upload `gdk::Texture` to GTK thread.

## Review Focus
1. Non-blocking UI: Heavy network requests or image decoding must never stall the GTK main loop.
2. Graceful Error Handling: Empty search results, rate limits, or network errors must display clean in-app toasts/status banners rather than panic.
3. Clean MPV Launching: Subprocess execution must properly detach so child process life-cycle does not lock the application.
4. Thumbnail Caching: Repeated searches for the same videos should load cached thumbnail files from `~/.cache/youtube-rusty/`.
5. Wayland Native: Proper application ID (`com.kunal.YouTubeRusty`) and window scaling under SwayFX.

---

### Task 1: Project Scaffolding & Dependencies

**Files:**
- Create: `~/Projects/youtube-rusty/Cargo.toml`
- Create: `~/Projects/youtube-rusty/src/main.rs`
- Create: `~/Projects/youtube-rusty/.gitignore`

- [ ] **Step 1.1:** Initialize Cargo project in `~/Projects/youtube-rusty`.
- [ ] **Step 1.2:** Configure `Cargo.toml` with `gtk4`, `libadwaita`, `tokio`, `rustypipe`, `reqwest`, `serde`, and `directories`.
- [ ] **Step 1.3:** Setup `.gitignore` ignoring `/target` and temporary cache files.
- [ ] **Step 1.4:** Verify `cargo check` passes.
- [ ] **Step 1.5:** Commit changes to git with message `chore: initialize cargo project and dependencies`.

---

### Task 2: Libadwaita Base Application Window

**Files:**
- Create: `~/Projects/youtube-rusty/src/ui/mod.rs`
- Create: `~/Projects/youtube-rusty/src/ui/window.rs`
- Modify: `~/Projects/youtube-rusty/src/main.rs`

- [ ] **Step 2.1:** Implement `AdwApplication` initialization with application ID `com.kunal.YouTubeRusty`.
- [ ] **Step 2.2:** Build `AdwApplicationWindow` containing an `AdwHeaderBar`, a `GtkSearchEntry` for queries, and a placeholder `GtkScrolledWindow` with `GtkFlowBox`.
- [ ] **Step 2.3:** Add dark-theme hint via `AdwStyleManager`.
- [ ] **Step 2.4:** Run `cargo run` to verify window launches on SwayFX and closes cleanly.
- [ ] **Step 2.5:** Commit changes with message `feat(ui): add libadwaita application window with header and search entry`.

---

### Task 3: Background Backend Engine (RustyPipe Integration)

**Files:**
- Create: `~/Projects/youtube-rusty/src/backend/mod.rs`
- Create: `~/Projects/youtube-rusty/src/backend/engine.rs`
- Create: `~/Projects/youtube-rusty/src/backend/models.rs`
- Modify: `~/Projects/youtube-rusty/src/main.rs`

- [ ] **Step 3.1:** Define `VideoSummary` data struct (ID, title, channel name, duration, view count, thumbnail URL).
- [ ] **Step 3.2:** Implement async `search_videos(query)` and `fetch_trending()` functions using `rustypipe::RustyPipe`.
- [ ] **Step 3.3:** Setup `glib::MainContext::channel` bridging background Tokio worker events with GTK UI thread.
- [ ] **Step 3.4:** Add a test verifying `search_videos("test")` parses results cleanly.
- [ ] **Step 3.5:** Commit changes with message `feat(backend): implement async rustypipe query worker`.

---

### Task 4: Video Card Widget & Thumbnail Caching

**Files:**
- Create: `~/Projects/youtube-rusty/src/ui/video_card.rs`
- Create: `~/Projects/youtube-rusty/src/backend/cache.rs`
- Modify: `~/Projects/youtube-rusty/src/ui/window.rs`

- [ ] **Step 4.1:** Build `VideoCard` widget displaying thumbnail image, title label, channel label, and duration badge.
- [ ] **Step 4.2:** Implement disk-backed thumbnail cache in `~/.cache/youtube-rusty/thumbnails/`.
- [ ] **Step 4.3:** Fetch images asynchronously and convert bytes to `gdk::Texture` without blocking UI.
- [ ] **Step 4.4:** Populate the `GtkFlowBox` with search result cards upon submitting search query.
- [ ] **Step 4.5:** Commit changes with message `feat(ui): add video card widget with async thumbnail caching`.

---

### Task 5: MPV Player Launcher Integration

**Files:**
- Create: `~/Projects/youtube-rusty/src/player/mod.rs`
- Create: `~/Projects/youtube-rusty/src/player/mpv.rs`
- Modify: `~/Projects/youtube-rusty/src/ui/video_card.rs`
- Modify: `~/Projects/youtube-rusty/src/ui/window.rs`

- [ ] **Step 5.1:** Implement `play_video(video_id, title)` using `std::process::Command` to invoke `/usr/bin/mpv`.
- [ ] **Step 5.2:** Configure detached child process spawning (inherits user's `mpv.conf` with 360p default and ModernZ OSD).
- [ ] **Step 5.3:** Connect card activation / double-click event to `play_video`.
- [ ] **Step 5.4:** Show an in-app `AdwToast` notification: *"Opening in MPV..."*.
- [ ] **Step 5.5:** Commit changes with message `feat(player): connect video card click to detached mpv launcher`.

---

### Task 6: Polish, Adaptive Layout & Shortcuts

**Files:**
- Modify: `~/Projects/youtube-rusty/src/ui/window.rs`
- Modify: `~/Projects/youtube-rusty/src/ui/video_card.rs`
- Create: `~/Projects/youtube-rusty/README.md`

- [ ] **Step 6.1:** Add keyboard navigation: `/` focuses the search bar, `Esc` clears/unfocuses.
- [ ] **Step 6.2:** Add responsive grid scaling (automatically adjusts card column count on window resize).
- [ ] **Step 6.3:** Test full workflow end-to-end on Wayland/SwayFX desktop.
- [ ] **Step 6.4:** Write `README.md` with build instructions.
- [ ] **Step 6.5:** Commit changes with message `chore: polish UI shortcuts, responsive sizing, and documentation`.
