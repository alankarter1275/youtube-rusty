# Bug Fixes and Design Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the hidden search bar, 1-column stretched video card grid, GtkPopover finalization memory warnings, and YouTube Trending 400 bad request error, while fully aligning the UI layout and icons with the Kalam Terracotta design mockups in `design/youtube-rusty design-png/`.

**Architecture:** Libadwaita / GTK4 application with RustyPipe engine backend, external MPV IPC session client, and Dark Terracotta palette.

**Tech Stack:** Rust, GTK4, Libadwaita, RustyPipe 0.11, MPV IPC.

**Spec:** `design/DESIGN_SPEC.md` and `design/youtube-rusty design-png/`

## Global Constraints
- Dark Terracotta palette (`#100C0A` background, `#1A1512` surface, `#D4552A` accent, `#F4EDE7` text).
- 4-column adaptive video grid at desktop resolution; cards fixed at 280px width with 16:9 thumbnails.
- Symbolic icons only; no emoji.
- Clean GTK widget lifecycle with zero unparent/finalization warnings.

---

### Task 1: Fix HeaderBar Search Bar Visibility & Alignment
**Files:**
- Modify: `src/ui/window.rs:65-95`
- [ ] Remove `header_bar.set_show_title(false)` so `search_clamp` and `search_entry` are visible.
- [ ] Ensure search entry placeholder is `"Search YouTube"` and has `.yr-search` styling.

### Task 2: Fix 4-Column Grid Geometry & Video Card Width Clamping
**Files:**
- Modify: `src/ui/video_card.rs:18-85`
- Modify: `src/ui/window.rs:165-180`, `src/ui/window.rs:1095-1115`
- [ ] In `VideoCard::new_grid`:
  - Set `title_label.set_natural_wrap_mode(gtk4::NaturalWrapMode::None)`.
  - Set `title_label.set_width_request(220)`.
  - Set `root_box.set_size_request(280, -1)` and `root_box.set_halign(Align::Start)`.
  - Set `container.set_halign(Align::Start)`.
  - Set `picture.set_size_request(280, 158)` and `overlay.set_size_request(280, 158)`.
- [ ] In `MainWindow`:
  - Set `flow_box.set_homogeneous(true)` in desktop grid mode so 4 columns are strictly maintained without cards stretching to 100% width.

### Task 3: Fix `GtkPopover` Finalization Warning
**Files:**
- Modify: `src/ui/details_sheet.rs:260-390`
- [ ] In `ContextMenu::attach`:
  - Connect `target.connect_unroot(move |_| popover.unparent())` so when cards are removed from `flow_box`, the attached popover is explicitly unparented before `FlowBoxChild` finalization.

### Task 4: Fix Trending Feed Error with Search Fallback
**Files:**
- Modify: `src/backend/engine.rs:35-46`
- [ ] In `Engine::trending`:
  - Wrap `self.rp.query().trending().await` in error recovery. If YouTube returns 400 Bad Request, automatically fall back to `self.search("trending").await` so the homepage always populates valid video cards.

### Task 5: Align Visual Design with Mockups
**Files:**
- Modify: `src/ui/style.css`
- Modify: `src/ui/rail.rs:25-60`
- Modify: `src/ui/video_card.rs:170-200`
- Modify: `src/ui/window.rs`
- [ ] In `style.css`: Add `.yr-logo` with `@yr_accent` background, border-radius 10px, 40x40 size, matching `Home – player card.png`.
- [ ] In `rail.rs`:
  - Use `video-display-symbolic` for Subscriptions (replace `emblem-favorite-symbolic`).
  - Use `multimedia-equalizer-symbolic` for Settings (replace `emblem-system-symbolic`).
- [ ] In `video_card.rs`:
  - Align horizontal search card to mockup: Title → Views · Age → Avatar + Channel name → Description snippet.
- [ ] In `window.rs`:
  - Set Subscriptions section heading to `"Latest"`.
  - Set History page heading to `"History"`.
  - Render Settings / Preferences as an in-window page with header search and rail visible.

### Task 6: Verification & Live Screenshots
- [ ] Run `cargo check` and `cargo build`.
- [ ] Launch `youtube-rusty` in Sway, verify 4-column layout, search bar, and zero GTK terminal warnings.
- [ ] Capture verification screenshots and confirm visual match against mockups.
