//! Optional, privacy-preserving runtime diagnostics for the user's real device.
//! All labels are static: do not put titles, URLs, search text, or IDs in logs.
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

static ENABLED: OnceLock<bool> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static MPV_PID: AtomicU32 = AtomicU32::new(0);
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
static ACTIVITIES: OnceLock<Mutex<Vec<(u64, &'static str, Instant)>>> = OnceLock::new();
static FRAME_PENDING: OnceLock<Mutex<VecDeque<(&'static str, Instant)>>> = OnceLock::new();

pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("YOUTUBE_RUSTY_DIAGNOSTICS").is_some_and(|v| v != "0"))
}

pub fn start() {
    if enabled() {
        START.get_or_init(Instant::now);
        log(format_args!("session_start clock=main_start units=ms,kib privacy=static_labels"));
    }
}

fn elapsed_ms(start: Instant) -> f64 { start.elapsed().as_secs_f64() * 1000.0 }

fn log(args: std::fmt::Arguments<'_>) {
    if !enabled() { return; }
    let ms = START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0;
    // A closed or redirected terminal must never crash the app.
    let _ = writeln!(std::io::stderr().lock(), "[rusty-diag] t_ms={ms:.1} {args}");
}

pub fn milestone(label: &'static str) {
    log(format_args!("event=milestone name={label}"));
}

/// Each async operation has its own ID; dropping it early reports cancellation.
#[must_use]
pub struct Span {
    label: &'static str,
    started: Option<Instant>,
    id: u64,
    parent_id: u64,
}
impl Span {
    pub fn new(label: &'static str) -> Self {
        if enabled() {
            Self { label, started: Some(Instant::now()), id: NEXT_ID.fetch_add(1, Ordering::Relaxed), parent_id: 0 }
        } else {
            Self { label, started: None, id: 0, parent_id: 0 }
        }
    }
    pub fn id(&self) -> u64 { self.id }
    pub fn child(label: &'static str, parent_id: u64) -> Self {
        let mut span = Self::new(label);
        span.parent_id = parent_id;
        span
    }
    pub fn finish(mut self, status: &'static str) {
        if let Some(started) = self.started.take() {
            log(format_args!("event=span name={} id={} parent_id={} elapsed_ms={:.1} status={status}", self.label, self.id, self.parent_id, elapsed_ms(started)));
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            log(format_args!("event=span name={} id={} parent_id={} elapsed_ms={:.1} status=cancelled_or_abandoned", self.label, self.id, self.parent_id, elapsed_ms(started)));
        }
    }
}

/// UI-thread activity name for the stall watchdog; nested activities are supported.
#[must_use]
pub struct Activity { id: u64, span: Option<Span> }
impl Activity {
    pub fn new(label: &'static str) -> Self {
        if !enabled() { return Self { id: 0, span: None }; }
        let span = Span::new(label);
        let id = span.id;
        ACTIVITIES.get_or_init(|| Mutex::new(Vec::new())).lock()
            .unwrap_or_else(|e| e.into_inner()).push((id, label, Instant::now()));
        Self { id, span: Some(span) }
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        if let Some(span) = self.span.take() {
            let mut activities = ACTIVITIES.get_or_init(|| Mutex::new(Vec::new())).lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(i) = activities.iter().position(|(id, _, _)| *id == self.id) {
                activities.remove(i);
            }
            drop(activities);
            span.finish("finished");
        }
    }
}

fn current_activity() -> Option<(&'static str, Duration)> {
    ACTIVITIES.get_or_init(|| Mutex::new(Vec::new())).lock()
        .unwrap_or_else(|e| e.into_inner()).last().map(|(_, label, start)| (*label, start.elapsed()))
}

// These probes hold only weak references and never keep a card or image alive.
// Track just main-flow VideoCards (not skeletons, search channel cards, or channel pages).
const MAX_TRACKED_CARDS: usize = 4096;
const MAX_CARD_SAMPLE: usize = 256;
const MAX_PENDING_CARD_BATCHES: usize = 16;

#[derive(Clone)]
struct CardProbe {
    container: gtk4::glib::WeakRef<gtk4::FlowBoxChild>,
    root: gtk4::glib::WeakRef<gtk4::Box>,
    picture: gtk4::glib::WeakRef<gtk4::Picture>,
}

#[derive(Default, Debug, PartialEq, Eq)]
struct CardCounts {
    containers_alive: usize,
    containers_parented: usize,
    roots_alive: usize,
    pictures_alive: usize,
}
fn card_counts(probes: &[CardProbe]) -> CardCounts {
    let mut counts = CardCounts::default();
    for probe in probes {
        if let Some(container) = probe.container.upgrade() {
            counts.containers_alive += 1;
            if container.parent().is_some() { counts.containers_parented += 1; }
        }
        if probe.root.upgrade().is_some() { counts.roots_alive += 1; }
        if probe.picture.upgrade().is_some() { counts.pictures_alive += 1; }
    }
    counts
}

#[derive(Default)]
pub struct CardTracker {
    cards: RefCell<Vec<CardProbe>>,
    omitted: Cell<usize>,
    pending: Rc<Cell<usize>>,
}
impl CardTracker {
    pub fn track(&self, card: &crate::ui::video_card::VideoCard) {
        if !enabled() { return; }
        let mut cards = self.cards.borrow_mut();
        if cards.len() >= MAX_TRACKED_CARDS {
            self.omitted.set(self.omitted.get().saturating_add(1));
            return;
        }
        cards.push(CardProbe {
            container: card.container.downgrade(),
            root: card.highlight_target.downgrade(),
            picture: card.picture.downgrade(),
        });
    }

    /// Called immediately before removing main-flow children. Only cards currently
    /// parented to this FlowBox are sampled; replacements removed earlier are excluded.
    pub fn before_clear(&self, flow: &gtk4::FlowBox) {
        if !enabled() { return; }
        let mut cards = self.cards.borrow_mut();
        let mut eligible = 0usize;
        let mut sample = Vec::new();
        for card in cards.drain(..) {
            let attached = card.container.upgrade()
                .and_then(|container| container.parent())
                .map(|parent| parent == *flow.upcast_ref::<gtk4::Widget>())
                .unwrap_or(false);
            if attached {
                eligible += 1;
                if sample.len() < MAX_CARD_SAMPLE { sample.push(card); }
            }
        }
        let omitted = self.omitted.replace(0);
        if eligible == 0 && omitted == 0 { return; }
        let batch = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if self.pending.get() >= MAX_PENDING_CARD_BATCHES {
            log(format_args!("event=card_lifetime batch={batch} status=skipped_pending_limit tracked_attached={eligible} untracked_created={omitted}"));
            return;
        }
        self.pending.set(self.pending.get() + 1);
        log(format_args!("event=card_lifetime batch={batch} status=scheduled tracked_attached={eligible} sampled={} untracked_created={omitted}", sample.len()));
        let sample = Rc::new(sample);
        let started = Instant::now();
        for delay in [5, 30] {
            let sample = sample.clone();
            let pending = self.pending.clone();
            gtk4::glib::timeout_add_local_once(Duration::from_secs(delay), move || {
                let counts = card_counts(&sample);
                log(format_args!("event=card_lifetime batch={batch} status=sample nominal_delay_s={delay} elapsed_ms={:.1} sampled={} containers_alive={} containers_parented={} roots_alive={} pictures_alive={}",
                    elapsed_ms(started), sample.len(), counts.containers_alive,
                    counts.containers_parented, counts.roots_alive, counts.pictures_alive));
                if delay == 30 { pending.set(pending.get().saturating_sub(1)); }
            });
        }
    }
}

/// Mark a UI change for the next observed frame-clock paint (not physical display).
pub fn next_frame(label: &'static str) {
    if !enabled() { return; }
    let mut pending = FRAME_PENDING.get_or_init(|| Mutex::new(VecDeque::new())).lock()
        .unwrap_or_else(|e| e.into_inner());
    if pending.len() == 64 { pending.pop_front(); }
    pending.push_back((label, Instant::now()));
}

pub fn install_frame_probe(window: &gtk4::Window) {
    if !enabled() { return; }
    let Some(clock) = window.frame_clock() else {
        log(format_args!("event=frame_probe status=unavailable"));
        return;
    };
    let paint_start = std::rc::Rc::new(std::cell::Cell::new(None::<Instant>));
    let before = paint_start.clone();
    clock.connect_before_paint(move |_| before.set(Some(Instant::now())));
    let weak = window.downgrade();
    let first = std::cell::Cell::new(true);
    clock.connect_after_paint(move |_| {
        let Some(before) = paint_start.take() else { return; };
        if first.replace(false) {
            milestone("first_frame_after_paint");
            if let Some(window) = weak.upgrade() {
                if let Some(renderer) = window.renderer() {
                    log(format_args!("event=renderer backend={}", renderer.type_().name()));
                }
            }
        }
        let paint_ms = elapsed_ms(before);
        let pending = {
            let mut q = FRAME_PENDING.get_or_init(|| Mutex::new(VecDeque::new())).lock()
                .unwrap_or_else(|e| e.into_inner());
            q.drain(..).collect::<Vec<_>>()
        };
        for (label, started) in pending {
            let wait = before.saturating_duration_since(started).as_secs_f64() * 1000.0;
            log(format_args!("event=frame name={label} wait_to_before_paint_ms={wait:.1} paint_phase_ms={paint_ms:.1}"));
        }
        if paint_ms >= 50.0 { log(format_args!("event=slow_paint paint_phase_ms={paint_ms:.1}")); }
    });
}

#[derive(Default, Debug, PartialEq, Eq)]
struct ProcSample { rss_kib: Option<u64>, peak_kib: Option<u64>, anon_kib: Option<u64>, file_kib: Option<u64>, threads: Option<u64>, cpu_ticks: Option<u64>, read_bytes: Option<u64>, write_bytes: Option<u64> }
fn named_number(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key { return None; }
        value.split_whitespace().next()?.parse().ok()
    })
}
fn cpu_ticks(stat: &str) -> Option<u64> {
    // comm is parenthesized and may contain spaces or parentheses; the final ')' ends it.
    let after = stat.rsplit_once(')')?.1;
    let fields: Vec<_> = after.split_whitespace().collect();
    let user: u64 = fields.get(11)?.parse().ok()?; // field 14
    let system: u64 = fields.get(12)?.parse().ok()?; // field 15
    Some(user.saturating_add(system))
}
fn parse_proc(status: &str, stat: &str, io: &str) -> ProcSample {
    ProcSample {
        rss_kib: named_number(status, "VmRSS"), peak_kib: named_number(status, "VmHWM"),
        anon_kib: named_number(status, "RssAnon"), file_kib: named_number(status, "RssFile"),
        threads: named_number(status, "Threads"), cpu_ticks: cpu_ticks(stat),
        read_bytes: named_number(io, "read_bytes"), write_bytes: named_number(io, "write_bytes"),
    }
}
fn number(value: Option<u64>) -> String { value.map(|v| v.to_string()).unwrap_or_else(|| "unavailable".into()) }
fn sample_process() {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let io = std::fs::read_to_string("/proc/self/io").unwrap_or_default();
    let p = parse_proc(&status, &stat, &io);
    log(format_args!("event=process rss_kib={} peak_kib={} rss_anon_kib={} rss_file_kib={} threads={} cpu_ticks_total={} io_read_bytes_total={} io_write_bytes_total={}", number(p.rss_kib), number(p.peak_kib), number(p.anon_kib), number(p.file_kib), number(p.threads), number(p.cpu_ticks), number(p.read_bytes), number(p.write_bytes)));
}

/// Scope identifies the child launched by this session. Never includes descendants.
pub struct MpvProcess(u32);
impl MpvProcess {
    pub fn set(pid: u32) -> Self {
        if enabled() { MPV_PID.store(pid, Ordering::Relaxed); }
        Self(pid)
    }
}
impl Drop for MpvProcess {
    fn drop(&mut self) { if enabled() { let _ = MPV_PID.compare_exchange(self.0, 0, Ordering::Relaxed, Ordering::Relaxed); } }
}
fn sample_mpv() {
    let pid = MPV_PID.load(Ordering::Relaxed);
    if pid == 0 { log(format_args!("event=mpv_process status=not_tracked")); return; }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    log(format_args!("event=mpv_process rss_kib={} peak_kib={} scope=launched_child_only", number(named_number(&status, "VmRSS")), number(named_number(&status, "VmHWM"))));
}

// Aggregate high-frequency image events instead of printing one line per thumbnail.
#[derive(Clone, Copy)]
pub enum ImageEvent { TextureHit, CompressedHit, DiskHit, Network, Coalesced, Decode, Delivered, DecodeTimeUs, NetworkTimeUs, DiskTimeUs }
static IMAGE: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
pub fn image_event(event: ImageEvent, value: u64) {
    if !enabled() { return; }
    IMAGE[event as usize].fetch_add(value, Ordering::Relaxed);
}
pub struct AggregateTimer { event: ImageEvent, start: Option<Instant> }
impl AggregateTimer {
    pub fn new(event: ImageEvent) -> Self { Self { event, start: enabled().then(Instant::now) } }
}
impl Drop for AggregateTimer {
    fn drop(&mut self) {
        if let Some(start) = self.start { IMAGE[self.event as usize].fetch_add(start.elapsed().as_micros().min(u64::MAX as u128) as u64, Ordering::Relaxed); }
    }
}
pub fn image_summary(textures: usize, compressed_bytes: Option<usize>, inflight: usize, ready: usize) {
    if !enabled() { return; }
    let a = |i: usize| IMAGE[i].load(Ordering::Relaxed);
    log(format_args!("event=images textures_cached={textures} compressed_bytes={} inflight={inflight} ready={ready} texture_hits={} compressed_hits={} disk_hits={} network_attempts={} coalesced={} decodes={} delivered={} decode_us_total={} network_us_total={} disk_us_total={}", compressed_bytes.map(|v| v.to_string()).unwrap_or_else(|| "busy".into()), a(0), a(1), a(2), a(3), a(4), a(5), a(6), a(7), a(8), a(9)));
}

static ATTR_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static ATTR_STARTED: AtomicU64 = AtomicU64::new(0);
static ATTR_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub struct AttributionRequest(bool);
impl AttributionRequest {
    pub fn new() -> Self {
        let on = enabled();
        if on { ATTR_ACTIVE.fetch_add(1, Ordering::Relaxed); ATTR_STARTED.fetch_add(1, Ordering::Relaxed); }
        Self(on)
    }
}
impl Drop for AttributionRequest { fn drop(&mut self) { if self.0 { ATTR_ACTIVE.fetch_sub(1, Ordering::Relaxed); } } }
pub fn attribution_cache_hit() { if enabled() { ATTR_CACHE_HITS.fetch_add(1, Ordering::Relaxed); } }
pub fn ui_summary(view: &'static str, cards: usize, video_count: usize, search_count: usize, pictures: usize, avatars: usize, channel_pages: usize, attribution_entries: Option<usize>) {
    if !enabled() { return; }
    let cache = attribution_entries.map(|n| n.to_string()).unwrap_or_else(|| "busy".into());
    log(format_args!("event=ui view={view} mounted_cards={cards} video_items={video_count} search_items={search_count} picture_refs={pictures} avatar_refs={avatars} channel_pages={channel_pages} attribution_cache_entries={cache} attribution_active={} attribution_started_total={} attribution_cache_hits_total={}", ATTR_ACTIVE.load(Ordering::Relaxed), ATTR_STARTED.load(Ordering::Relaxed), ATTR_CACHE_HITS.load(Ordering::Relaxed)));
}

/// A 50 ms GTK heartbeat and one background watcher. The reported duration is
/// a conservative lower bound, not a backtrace or definitive root cause.
pub fn install_monitor(window: &gtk4::Window) {
    if !enabled() { return; }
    let alive = Arc::new(AtomicBool::new(true));
    let on_close = alive.clone();
    window.connect_destroy(move |_| on_close.store(false, Ordering::Relaxed));
    let heartbeat_alive = alive.clone();
    gtk4::glib::timeout_add_local(Duration::from_millis(50), move || {
        if !heartbeat_alive.load(Ordering::Relaxed) { return gtk4::glib::ControlFlow::Break; }
        HEARTBEAT.fetch_add(1, Ordering::Relaxed);
        gtk4::glib::ControlFlow::Continue
    });
    let _ = std::thread::Builder::new().name("yr-diagnostics".into()).spawn(move || {
        let mut prior = HEARTBEAT.load(Ordering::Relaxed);
        let mut last_advance = Instant::now();
        let mut seen = false;
        let mut reported = false;
        let mut next_sample = Instant::now();
        while alive.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            let beat = HEARTBEAT.load(Ordering::Relaxed);
            if beat != prior {
                if reported { log(format_args!("event=stall status=ended at_least_ms={}", last_advance.elapsed().saturating_sub(Duration::from_millis(50)).as_millis())); }
                prior = beat; last_advance = Instant::now(); seen = true; reported = false;
            } else if seen && !reported && last_advance.elapsed() >= Duration::from_millis(150) {
                if let Some((label, duration)) = current_activity() {
                    log(format_args!("event=stall status=ongoing at_least_ms={} activity={label} activity_ms={:.1}", last_advance.elapsed().saturating_sub(Duration::from_millis(50)).as_millis(), duration.as_secs_f64()*1000.0));
                } else {
                    log(format_args!("event=stall status=ongoing at_least_ms={} activity=unknown", last_advance.elapsed().saturating_sub(Duration::from_millis(50)).as_millis()));
                }
                reported = true;
            }
            if next_sample.elapsed() >= Duration::from_secs(30) { sample_process(); sample_mpv(); next_sample = Instant::now(); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proc_fields_and_cpu_with_spaces_in_name() {
        let s = parse_proc("VmRSS:\t1234 kB\nVmHWM: 2200 kB\nRssAnon: 500 kB\nRssFile: 700 kB\nThreads: 5\n", "42 (name with spaces) S 1 2 3 4 5 6 7 8 9 10 120 34 0 0", "read_bytes: 1024\nwrite_bytes: 2048\n");
        assert_eq!(s.rss_kib, Some(1234)); assert_eq!(s.peak_kib, Some(2200));
        assert_eq!(s.cpu_ticks, Some(154)); assert_eq!(s.write_bytes, Some(2048));
    }
    #[test]
    fn malformed_and_missing_proc_data_are_unknown() {
        let s = parse_proc("VmRSS: nope\n", "invalid", "read_bytes: xyz");
        assert_eq!(s.rss_kib, None); assert_eq!(s.cpu_ticks, None); assert_eq!(s.read_bytes, None);
    }
    // A display is required; CI runs this separately under Xvfb. The control
    // verifies both the probe and a card with real interaction handlers release
    // their widgets. It also exercises each handler before removing the card.
    #[test]
    #[ignore = "requires a GTK display"]
    fn inspect_real_card_lifetimes() {
        gtk4::init().expect("GTK display is required");
        libadwaita::init().expect("initialize libadwaita");
        let summary = crate::backend::models::VideoSummary {
            id: String::new(), title: "Test".into(), channel_id: String::new(),
            channel_name: String::new(), channel_avatar_url: String::new(),
            duration_seconds: None, duration_text: String::new(), view_count_text: String::new(),
            view_count: None, published_text: String::new(), published_unix: None,
            is_short: false, thumbnail_url: String::new(), description_snippet: String::new(),
            collaborators: vec![],
        };
        let sample = |interactions: bool| {
            let flow = gtk4::FlowBox::new();
            let card = crate::ui::video_card::VideoCard::new_grid(&summary);
            if interactions { card.connect_card_interactions(|_| {}, || {}, || {}); }
            let probe = CardProbe {
                container: card.container.downgrade(),
                root: card.highlight_target.downgrade(),
                picture: card.picture.downgrade(),
            };
            flow.append(&card.container);
            flow.remove(&card.container);
            drop(card);
            probe
        };
        // Emit the GTK signals on a live card to verify selection, play, and
        // details callbacks still work without strong widget captures.
        let live = crate::ui::video_card::VideoCard::new_grid(&summary);
        let selections = Rc::new(Cell::new(0));
        let plays = Rc::new(Cell::new(0));
        let details = Rc::new(Cell::new(0));
        let s = selections.clone();
        let p = plays.clone();
        let d = details.clone();
        live.connect_card_interactions(move |_| s.set(s.get() + 1), move || p.set(p.get() + 1), move || d.set(d.get() + 1));
        let controllers = live.click_target.observe_controllers();
        for i in 0..controllers.n_items() {
            let Some(controller) = controllers.item(i) else { continue; };
            if let Ok(gesture) = controller.clone().downcast::<gtk4::GestureClick>() {
                match gesture.button() {
                    1 => {
                        gesture.emit_by_name::<()>("pressed", &[&1i32, &0f64, &0f64]);
                        gesture.emit_by_name::<()>("pressed", &[&2i32, &0f64, &0f64]);
                    }
                    2 => gesture.emit_by_name::<()>("released", &[&1i32, &0f64, &0f64]),
                    3 => gesture.emit_by_name::<()>("pressed", &[&1i32, &0f64, &0f64]),
                    _ => {}
                }
            } else if let Ok(gesture) = controller.downcast::<gtk4::GestureLongPress>() {
                gesture.emit_by_name::<()>("pressed", &[&0f64, &0f64]);
            }
        }
        assert_eq!(selections.get(), 5);
        assert_eq!(plays.get(), 2);
        assert_eq!(details.get(), 2);
        drop(controllers);
        drop(live);

        let control = sample(false);
        let interactions = sample(true);
        assert_eq!(card_counts(std::slice::from_ref(&control)), CardCounts::default(),
            "the weak probe must not retain a removed card");
        let main_loop = gtk4::glib::MainLoop::new(None, false);
        let quit = main_loop.clone();
        gtk4::glib::timeout_add_local_once(Duration::from_secs(5), move || quit.quit());
        main_loop.run();
        let control = card_counts(&[control]);
        let with_interactions = card_counts(&[interactions]);
        assert_eq!(control, CardCounts::default(), "control must stay released after GTK events run");
        assert_eq!(with_interactions, CardCounts::default(), "removed interactive cards must release their widgets");
        eprintln!("CARD_LIFETIME_AFTER_5S_CONTROL={control:?} CARD_LIFETIME_AFTER_5S_WITH_INTERACTIONS={with_interactions:?}");
    }

}
