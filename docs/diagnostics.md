# Opt-in performance diagnostics

Diagnostics are off by default. On Linux, start a new instance from the repository in Fish:

```fish
env YOUTUBE_RUSTY_DIAGNOSTICS=1 cargo run --release
```

Close any existing Rusty instance first, since GTK may hand activation to it instead of starting a new process. Use the app normally, then close it. Lines beginning `[rusty-diag]` appear in the terminal. To save **all** terminal output (including other software's messages) for review instead, use:

```fish
env YOUTUBE_RUSTY_DIAGNOSTICS=1 cargo run --release > rusty-run.log 2>&1
```

Review a log before sharing it: Rusty's diagnostic lines use fixed labels and numerical measurements, not titles, searches, channel/video IDs or URLs. Other application, GTK, Cargo or error messages are not controlled by this diagnostic privacy rule and may contain sensitive information. The log file is not automatically deleted. Logging itself adds overhead, especially if the terminal or disk is slow.

## Reading the output

* `t_ms` is time since the start of this process. Spans have distinct IDs and report durations and status; stale/cancelled actions are not successful loads. Backend durations include any internal caching and processing, not just network time. Action and backend spans are independent and are not directly linked by ID.
* Navigation, selected result builds, channel reloads and playback handoff are measured. Frame records measure the wait until GTK's next before-paint and the before/after-paint interval. These are approximate frame-clock phases, not GPU timings or proof a frame appeared on the physical display. The first observed frame is after the probe is installed, not necessarily the first frame ever produced. Up to 64 pending frame markers are retained; old markers can be dropped.
* Every 30 seconds `process` reports the Rusty process only: Linux `/proc` RSS, peak RSS, anonymous and file-backed portions, thread count, cumulative CPU ticks, and cumulative physical read/write bytes when available. These are not CPU percentages, and shared mappings and GPU memory do not have simple ownership. `mpv_process` reports the launched player child separately, when tracked; it excludes its descendants. Missing values are `unavailable`, and a process can exit between sampling and reading. `not_tracked` does not assert that no external player exists.
* `ui` reports main-flow mounted children (including any non-card children), stored item and weak-reference counts, open channel pages, attribution cache entries, and active/started watch-page enrichment requests. A `busy` cache count means it could not be read without waiting. These counts are not memory sizes and do not include every GTK object or channel-page widget.
* `images` reports lookup-cache entries, compressed-cache bytes, inflight keys, ready queue length, and cumulative activity and durations. These omit textures still held by widgets, GPU allocations, and other allocations. `delivered` counts callbacks, including failures; decode counts are attempts. Timings are cumulative work durations, not elapsed session time, and concurrent work may overlap. Disk/network times include associated async waits. The bounded disk-writer queue and actual GPU use are not measured.
* The watchdog samples a 50 ms GTK heartbeat from a background thread. A stall is a conservative indication of a delayed heartbeat, not a stack trace or proof of which operation caused it. Short stalls, suspend/resume, scheduling delays, shutdown, and a covered window can complicate interpretation. The reported activity is the currently instrumented main-thread scope, if any.
* MPV logs child spawn, IPC connection, bridge readiness, stream-resolution duration and file-loaded events. Events from different requests may overlap; `file-loaded` does not establish physical playback or a reliable click-to-video latency. MPV child memory is never included in Rusty RSS.

Growth alone does not prove a leak: caches, live widgets, async work, GTK, graphics drivers and allocator retention can all contribute. Compare the same browsing steps over several cycles, allow the app to settle, and compare Rusty RSS, peak and live counts; peak RSS cannot decrease. No cache limits or saved data are changed by enabling diagnostics.
