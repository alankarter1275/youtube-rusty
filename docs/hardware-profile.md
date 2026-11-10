# Hardware profile

## User-reported device (2026-11-08)

This snapshot was provided by the user from their Arch Linux laptop. Treat it as the target platform when later discussing or testing performance changes; it is not a benchmark and the memory figures are momentary.

- **CPU:** Intel Pentium Silver N5030, x86_64, 4 cores / 4 threads, 1.10 GHz base, reported max 3.10 GHz.
- **RAM:** 3.6 GiB installed; at capture, 1.3 GiB available.
- **Swap:** 3.6 GiB zram; 773 MiB used at capture (this alone does not prove active memory thrashing).
- **Storage:** 931.5 GiB WDC WD10SPZX rotational HDD (`ROTA=1`); zram also listed as a block device.
- **Graphics:** Intel Gemini Lake UHD Graphics 605, `i915` driver; integrated/shared-memory GPU.
- **OS / session:** Arch Linux, Wayland, SwayFX (`sway:wlroots:swayfx`).
- **MPV:** v0.41.0-dirty, built 2026-11-03.

When evaluating performance, prioritize measured playback stability, scrolling responsiveness, RSS/available memory, and HDD activity on this device. Avoid treating speculative estimates as measured results.
