# youtube-rusty — Design Spec

Visual reference: 15 screens on a design canvas (the user will give you screenshots).
Palette/shape come from the Kalam design system. **Dark only for now** (Terracotta). Keep `style.css` as the single stylesheet.

## 1. Tokens
| Token | Value | Use |
|---|---|---|
| bg | `#100C0A` | window, header, sidebar (all the same) |
| surface | `#1A1512` | player card, popovers, sheet |
| text / text-2 | `#F4EDE7` / `#C9BEB6` | titles / metadata |
| accent | `#D4552A` | selected rail item, primary buttons, progress |
| link | `#E8845F` | channel name links |
| hairline | `rgba(232,221,213,.10)` | separators |
Fonts: **Playfair Display 500** for page titles and section headings only; **DM Sans** for everything else. Bundle both fonts (or fall back to serif / Adwaita Sans).
Radii: 6 (badges), 10 (inputs, menu items), 14 (thumbnails, groups), 22 (sheet, player card), pill (buttons, chips). No hover lift animation. Avoid large blurs; target is an Intel Gemini Lake GPU.

## 2. Shell
- Window: `AdwApplicationWindow` > `AdwToastOverlay` > `AdwToolbarView`. Top bar is an `AdwHeaderBar` with no border (blends with content). Search entry centered (`AdwClamp`, max ~420px). Primary menu button at the right (Preferences, Shortcuts, About).
- **Rail** (left, 68px, **same bg as content, no fill**): logo at top (28px from top; placeholder = terracotta rounded square with play triangle, replace later). **Home, Subscriptions, History** are **vertically centered**. **Settings** pinned to the bottom. Items are 44px circles; the selected one is filled with accent and has a dark icon. Separator: a 2px rounded line at the rail's right edge, inset 28px from top and 24px from bottom (it must not touch the window edges). Use symbolic icons only, no emoji.
- Content: `AdwNavigationView` so Channel / Search pages push, with back via header button, Alt+Left, Esc, and mouse back button.
- **Player card**: persistent overlay (`gtk::Overlay` over the whole content area, `halign=end`, `valign=end`, margin 20). Visible whenever something is playing; hidden otherwise.
- Narrow (< ~600px, use `AdwBreakpoint`): rail disappears; header shows search icon + menu; chips row scrolls; one column of cards; **bottom tab bar** with Home, Subscriptions, History, Settings (selected = accent circle); the player card becomes a slim bar above the tab bar (thumb, title, channel link, pause, close).

## 3. Screens
**Home** — chips row (Trending, Gaming, Music, News) then section title "Trending" (Playfair 18) then the video grid. Grid is **4 columns at the reference width**; make columns adapt (4 → 3 → 2 → 1) with window width. Use `GtkGridView` + `gio::ListStore` for virtualization once infinite scroll arrives (FlowBox is acceptable for v1).

**Video card** — 16:9 thumbnail (radius 14, `ContentFit::Cover`), duration badge bottom-right, below it a 36px circular `adw::Avatar` (image or initials), title (15px / 500, max 2 lines, ellipsis), one muted line `Channel · 19.8M views`. Click opens the **details sheet**; right-click / long-press opens the **context menu**.

**Details sheet** (replaces the details page; no comments, no related videos, no description page) — `AdwDialog` (bottom sheet when narrow), about 520px wide, radius 22: thumbnail (with duration), title (Playfair 22, 2 lines), channel row (avatar, channel name, views · age, "Open channel" button), **one line** of description (ellipsized), then a row with a big primary **Play in MPV** button and a segmented quality control (**360p default**, 720p, 1080p, Audio only), then four equal buttons: Add to queue, Watch later, Copy link, Download. Esc closes it.

**Context menu** (popover, 220px): Play in MPV, Add to queue, Watch later — separator — Copy link, Open channel, Download.

**Search results** — rows: 300px thumbnail left; right column has title (17px, 2 lines), `views · age`, channel row (24px avatar + name), 2-line description snippet. Top bar of the page: "Results for “term”" and a Filters button.

**Channel page** — no banner, no About tab. Header: 64px avatar, channel name (Playfair 26), line `@handle · 1.2M subscribers · 482 videos`, **Subscribe** primary button (local subscription, stored in `~/.config/youtube-rusty/`). Toolbar row: segmented **Videos | Shorts**, "Search in channel" entry, **Filter** button, **Sort: Newest** button. Below: the video grid (Videos tab) or a **6-column grid of 9:16 cards** (Shorts tab) with a **Play all** primary button in the toolbar.
- Filter popover: *Upload date* (Any time ✓, Today, This week, This month, This year), *Duration* (Any length ✓, Under 4 min, 4 to 20 min, Over 20 min), footer Reset + Apply.
- Sort popover: Newest ✓, Oldest, Most popular.
- Search/filter/sort apply to the active tab. Use RustyPipe's channel video/shorts tabs and its search-within-channel support if available (verify against the crate version in use).

**Subscriptions** — horizontal avatar strip (56px, channel name under each; click opens the channel) then section "Latest" with a video grid of recent uploads from subscribed channels.

**History** — title "History" + Clear history button; rows: 200px thumbnail, title, `channel · views`, "Watched today", and a remove (x) button. Stored locally; toggle in Settings.

**Settings (Preferences)** — `AdwPreferencesWindow` or a page with groups. Appearance: color scheme, accent color (Terracotta / Forest / Plum / Teal; only Terracotta needed now). Playback: default quality (360p), hardware decoding (VA-API) switch, player command (mpv). Data: clear thumbnail cache, save watch history switch.

**States** — Loading: same grid filled with skeleton cards (opacity pulse 900ms). Empty / error: `AdwStatusPage` styled as a 96px circle icon, "No results" title, one muted sentence, a primary action ("Clear search" / "Retry").

## 4. Player card (the core feature)
Overlay card, 370px wide, surface bg, radius 22, 1px hairline border, soft shadow.
**Collapsed:** 96px thumbnail; title (1 line, ellipsis); **channel name as a link** (accent-light, opens that channel page); `Queue · 2 of 5`; close (x) at the top right. Progress bar (4px, accent) with elapsed / total times. Controls: previous, **play/pause (accent circle)**, next, and a **Queue · N** button that expands the card.
**Expanded:** below the controls, a divider, a header line `5 videos · 1 h 42 min · drag to reorder` with **Shuffle** and **Clear** buttons, then the queue list (max about 5 rows visible, scroll the rest). Row: drag handle, 64px thumbnail, title, channel, remove (x). Current row tinted with accent at 16%. Rows reorder by drag and drop. Clicking a row jumps to it.
**Close (x):** stops playback, clears the card, and returns mpv to its idle start window.
**Shorts:** playing any short queues the whole Shorts list of that channel starting at the chosen one; previous/next move through it and the card shows `Short 3 of 12`.
**Add to queue** (details sheet, context menu) appends; **Play** replaces the queue with that single video (ask nothing, show a toast "Added to queue" when appending).

## 5. mpv integration for the card
- Launch mpv once per session with `--idle=yes --force-window=yes --input-ipc-server=<runtime-dir>/youtube-rusty-mpv.sock` (keep your existing hwdec / 360p / ModernZ options). Reuse the same instance for the whole queue.
- Talk JSON IPC over the Unix socket: `loadfile <url> replace|append`, `playlist-next`, `playlist-prev`, `playlist-move`, `playlist-remove`, `playlist-clear`, `set_property pause`, `stop` (for the close button).
- Observe properties for UI state: `time-pos`, `duration`, `pause`, `playlist-pos`, `playlist-count`, `idle-active`. Mirror the playlist in a `gio::ListStore` the UI binds to.
- If the socket is missing or mpv exits, hide the card and clear the store.
- Audio-only quality maps to `--no-video` / `--vid=no` for that file; other qualities map to ytdl-format selections.

## 6. Accessibility and polish
Every icon-only button has an accessible label (the reference has them). Text contrast ≥ 4.5:1. Keyboard: `/` or Ctrl+F focuses search, Esc closes sheet/popover, Alt+Left back, Space toggles pause when the card has focus.
