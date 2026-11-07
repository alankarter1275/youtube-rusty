# Prompt for the coding AI

You are updating **youtube-rusty** (Rust, gtk4 0.9, libadwaita 0.7, RustyPipe backend; mpv is the external player). I'm giving you a finished UI design as `DESIGN_SPEC.md`, a stylesheet `style.css`, and screenshots of every screen. **Follow them exactly; do not invent a different look.** Do not change the RustyPipe backend logic or the thumbnail cache pipeline except where noted.

## Rules
1. Replace the current `src/ui/style.css` with the provided `style.css`. Remove the old hardcoded styling. Use the CSS classes from the file (`yr-*`) on your widgets.
2. Dark scheme is forced (`PreferDark`). No emoji anywhere; use symbolic icons.
3. Work in the phases below. After each phase the app must compile and run. Show me a short summary and wait for my confirmation before moving on.
4. Performance matters (Intel Gemini Lake): no large blurs, no transform animations on grids, decode thumbnails at display size, plan for `GtkGridView` + `gio::ListStore`.
5. If something in the spec is ambiguous or conflicts with the backend, ask me one short question instead of guessing.

## Phases
1. **Shell**: `AdwToolbarView` + borderless header (centered search, primary menu), the 68px rail (logo, centered Home / Subscriptions / History, Settings at the bottom, accent circle for the selected item, inset 2px separator), `AdwNavigationView` for content.
2. **Home + cards**: chips row, section title, the adaptive grid (4 columns at wide widths), the new card with avatar, loading skeletons, empty / error states.
3. **Details sheet + context menu**: sheet with Play in MPV and quality choice (360p default, 720p, 1080p, Audio only), the secondary action buttons, right-click / long-press menu.
4. **mpv session + player card**: single mpv instance over IPC, the persistent overlay card (collapsed and expanded queue with drag-reorder, remove, shuffle, clear, close). Implement add-to-queue / play from the sheet and menu.
5. **Channel page**: header, Subscribe (local storage), Videos | Shorts tabs, in-channel search, Filter and Sort popovers, **Play all** for Shorts and the Shorts queue behavior.
6. **Search results page**, **Subscriptions**, **History** (local storage), **Settings**.
7. **Narrow layout** with `AdwBreakpoint`: bottom tab bar, slim player bar, single column.

Start with phase 1 now. Before writing code, list the files you will touch and the widget tree you plan for the shell.
