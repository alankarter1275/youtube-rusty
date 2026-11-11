use crate::backend::engine::Engine;
use crate::backend::settings::AppSettings;
use crate::player::mpv::Quality;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, Notify};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

const YR_BRIDGE_LUA: &str = r#"local utils = require 'mp.utils'
local yr_diagnostics = false

local function diagnostic_queue(stage, generation)
    if yr_diagnostics and generation then
        mp.commandv('script-message', 'yr-diagnostics', stage, tostring(generation))
    end
end

local core_ready = false
local req_counter = 0
local active_req_id = nil
local pending_hooks = {}
local title_by_url = {}

local function read_and_remove_json(path)
    if not path or path == '' then return nil end
    local f = io.open(path, 'rb')
    if not f then return nil end
    local raw = f:read('*a')
    f:close()
    os.remove(path)
    if not raw or raw == '' then return nil end
    return utils.parse_json(raw)
end

local function is_youtube_url(path)
    if not path then return false end
    return path:find('youtube%.com/watch%?v=') ~= nil
        or path:find('youtu%.be/') ~= nil
        or path:find('youtube%.com/shorts/') ~= nil
end

local function mark_ready()
    if not core_ready then
        core_ready = true
        mp.commandv('script-message', 'yr-bridge-ready')
    end
end

mp.register_event('idle', mark_ready)
mp.add_timeout(0.35, mark_ready)

mp.register_script_message('ping', function()
    if core_ready then
        mp.commandv('script-message', 'yr-bridge-ready')
    end
end)

mp.register_script_message('load-queue', function(json_path)
    local payload = read_and_remove_json(json_path)
    if not payload or not payload.items or #payload.items == 0 then return end
    diagnostic_queue('queue_received', payload.sync_gen)
    local items = payload.items
    local start_idx = tonumber(payload.start_index) or 0
    if start_idx < 0 then start_idx = 0 end
    if start_idx >= #items then start_idx = #items - 1 end

    title_by_url = {}
    for i = 1, #items do
        local it = items[i]
        if it.url and it.title and it.title ~= '' then
            title_by_url[it.url] = it.title
        end
    end

    mp.set_property('force-window', payload.audio_only and 'no' or 'yes')
    mp.set_property_bool('pause', false)

    if payload.m3u_path and payload.m3u_path ~= '' then
        mp.set_property('playlist-start', tostring(start_idx))
        diagnostic_queue('before_loadlist', payload.sync_gen)
        mp.commandv('loadlist', payload.m3u_path, 'replace')
        diagnostic_queue('after_loadlist', payload.sync_gen)
        os.remove(payload.m3u_path)
    end

    local cur_pos = mp.get_property_number('playlist-pos', -1)
    if cur_pos ~= start_idx then
        mp.set_property_number('playlist-pos', start_idx)
    end

    if payload.sync_gen then
        mp.commandv('script-message', 'yr-queue-synced', tostring(payload.sync_gen))
    end
end)

mp.register_script_message('append-queue', function(json_path)
    local payload = read_and_remove_json(json_path)
    if not payload or not payload.items or #payload.items == 0 then return end
    local items = payload.items

    for i = 1, #items do
        local it = items[i]
        if it.url and it.title and it.title ~= '' then
            title_by_url[it.url] = it.title
        end
    end

    if payload.m3u_path and payload.m3u_path ~= '' then
        mp.set_property('playlist-start', 'auto')
        mp.commandv('loadlist', payload.m3u_path, 'append')
        os.remove(payload.m3u_path)
    end
end)

mp.register_script_message('sync-queue', function(json_path)
    local payload = read_and_remove_json(json_path)
    if not payload or not payload.items or #payload.items == 0 then return end
    local items = payload.items

    title_by_url = {}
    for i = 1, #items do
        local it = items[i]
        if it.url and it.title and it.title ~= '' then
            title_by_url[it.url] = it.title
        end
    end

    mp.set_property('playlist-start', 'auto')
    mp.command('playlist-clear')

    if payload.before_m3u_path and payload.before_m3u_path ~= '' then
        local ok = mp.commandv('loadlist', payload.before_m3u_path, 'insert-at', '0')
        if not ok then
            local before_cnt = tonumber(payload.before_count) or 0
            mp.commandv('loadlist', payload.before_m3u_path, 'append')
            if before_cnt > 0 then
                mp.commandv('playlist-move', '0', tostring(before_cnt + 1))
            end
        end
        os.remove(payload.before_m3u_path)
    end

    if payload.after_m3u_path and payload.after_m3u_path ~= '' then
        mp.commandv('loadlist', payload.after_m3u_path, 'append')
        os.remove(payload.after_m3u_path)
    end

    if payload.sync_gen then
        mp.commandv('script-message', 'yr-queue-synced', tostring(payload.sync_gen))
    end
end)

mp.register_script_message('resolved', function(req_id, json_path)
    local hook = pending_hooks[req_id]
    pending_hooks[req_id] = nil
    local data = read_and_remove_json(json_path)

    if hook then
        if req_id == active_req_id and data then
            if data.ok then
                local cur_pos = mp.get_property_number('playlist-pos', -1)
                local pl_title = cur_pos >= 0
                    and mp.get_property('playlist/' .. tostring(cur_pos) .. '/title')
                    or nil
                local cur_path = mp.get_property('path', '')
                local media_title = (pl_title and pl_title ~= '') and pl_title
                    or (cur_path ~= '' and title_by_url[cur_path])
                    or data.title
                if media_title and media_title ~= '' then
                    if cur_path ~= '' and not title_by_url[cur_path] then
                        title_by_url[cur_path] = media_title
                    end
                    mp.set_property('file-local-options/force-media-title', media_title)
                end
                if data.user_agent and data.user_agent ~= '' then
                    mp.set_property('file-local-options/user-agent', data.user_agent)
                end
                if data.referer and data.referer ~= '' then
                    mp.set_property('file-local-options/referrer', data.referer)
                end
                mp.set_property('file-local-options/vid', data.is_audio_only and 'no' or 'auto')
                if not data.is_audio_only then
                    mp.set_property('force-window', 'yes')
                end
                if data.ytdl_json and data.ytdl_json ~= '' then
                    mp.set_property_native('user-data/mpv/ytdl/json-subprocess-result', {
                        status = 0,
                        stdout = data.ytdl_json,
                    })
                end
                mp.set_property('stream-open-filename', data.load_url)
            else
                mp.set_property('stream-open-filename', 'null://')
            end
        end
        hook:cont()
    end
end)

mp.add_hook('on_load', 10, function(hook)
    mp.set_property('playlist-start', 'auto')
    local path = mp.get_property('stream-open-filename', '')
    if not is_youtube_url(path) then return end

    local cur_pos = mp.get_property_number('playlist-pos', -1)
    local pl_title = cur_pos >= 0
        and mp.get_property('playlist/' .. tostring(cur_pos) .. '/title')
        or nil
    local known_title = (pl_title and pl_title ~= '') and pl_title or title_by_url[path]
    if known_title and known_title ~= '' then
        mp.set_property('file-local-options/force-media-title', known_title)
    end

    local ytdl_fmt = mp.get_property('file-local-options/ytdl-format', '')
    req_counter = req_counter + 1
    local req_id = tostring(req_counter)
    active_req_id = req_id
    pending_hooks[req_id] = hook
    hook:defer()
    mp.commandv('script-message', 'yr-resolve', req_id, path, ytdl_fmt or '')
end)

mp.add_hook('on_after_end_file', 50, function()
    mp.del_property('user-data/mpv/ytdl/json-subprocess-result')
end)
"#;

#[derive(Debug, Clone)]
pub struct MpvQueueItem {
    pub video_id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub enum MpvEvent {
    Loading,
    Loaded,
    TimePos(f64),
    Duration(f64),
    Pause(bool),
    PlaylistPos(usize),
    QueueSynced(u64),
    TrackEnded,
    Idle(bool),
    Error(String),
    ProcessExited,
}

#[derive(Clone, Copy)]
struct QueueTrace { generation: u64, trace_id: u64 }
struct QueuedCommand { body: String, trace: Option<QueueTrace> }

fn bridge_script(diagnostics: bool) -> String {
    if diagnostics {
        YR_BRIDGE_LUA.replacen("local yr_diagnostics = false", "local yr_diagnostics = true", 1)
    } else {
        YR_BRIDGE_LUA.to_owned()
    }
}

// Never trust an arbitrary script-message as a log label or print its payload.
fn diagnostic_queue_message(args: &[serde_json::Value]) -> Option<(&'static str, u64)> {
    let stage = match args.get(1)?.as_str()? {
        "queue_received" => "bridge_queue_received",
        "before_loadlist" => "bridge_before_loadlist",
        "after_loadlist" => "bridge_after_loadlist",
        _ => return None,
    };
    let generation = args.get(2)?.as_str()?.parse::<u64>().ok()?;
    if generation == 0 { return None; }
    Some((stage, generation))
}

pub struct MpvSession {
    ipc_dir: PathBuf,
    payload_seq: AtomicU64,
    default_quality: Arc<Mutex<Quality>>,
    cmd_sender: mpsc::UnboundedSender<QueuedCommand>,
    session_seq: u64,
    process: Arc<Mutex<Option<Child>>>,
    is_running: Arc<AtomicBool>,
}

fn youtube_watch_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

fn extract_youtube_id(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if let Some((_, query)) = trimmed.split_once("watch?") {
        for pair in query.split(['&', '#']) {
            if let Some(val) = pair.strip_prefix("v=") {
                let id = val.trim();
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    if let Some((_, after)) = trimmed.split_once("youtu.be/") {
        let id = after.split(['?', '&', '#', '/']).next().unwrap_or("").trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    if let Some((_, after)) = trimmed.split_once("/shorts/") {
        let id = after.split(['?', '&', '#', '/']).next().unwrap_or("").trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

fn sanitize_m3u_title(raw: &str, fallback_id: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut prev_space = false;
    for ch in raw.chars() {
        let mapped = if ch.is_control() { ' ' } else { ch };
        if mapped == ' ' {
            if !prev_space && !cleaned.is_empty() {
                cleaned.push(' ');
            }
            prev_space = true;
        } else {
            cleaned.push(mapped);
            prev_space = false;
        }
    }
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        if fallback_id.trim().is_empty() {
            "YouTube Video".to_string()
        } else {
            format!("YouTube ({})", fallback_id.trim())
        }
    } else {
        trimmed.to_string()
    }
}

fn build_m3u8_playlist(items: &[MpvQueueItem]) -> String {
    let mut out = String::with_capacity(16 + items.len() * 96);
    out.push_str("#EXTM3U\n");
    for it in items {
        let clean_title = sanitize_m3u_title(&it.title, &it.video_id);
        let url = youtube_watch_url(&it.video_id);
        out.push_str("#EXTINF:-1,");
        out.push_str(&clean_title);
        out.push('\n');
        out.push_str(&url);
        out.push('\n');
    }
    out
}

fn write_m3u8_file(ipc_dir: &Path, filename: &str, items: &[MpvQueueItem]) -> Option<PathBuf> {
    if items.is_empty() {
        return None;
    }
    let path = ipc_dir.join(filename);
    let content = build_m3u8_playlist(items);
    std::fs::write(&path, content.as_bytes()).ok()?;
    Some(path)
}

fn write_payload_json(ipc_dir: &Path, filename: &str, value: &serde_json::Value) -> Option<PathBuf> {
    let path = ipc_dir.join(filename);
    let bytes = serde_json::to_vec(value).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

impl MpvSession {
    /// Spawns a persistent `mpv` IPC process (`--ytdl=no`, zero Python / `yt-dlp`) paired with
    /// our lightweight `yr_bridge.lua` hook script so:
    /// 1. Every playlist entry keeps `https://www.youtube.com/watch?v=<id>` (enabling SponsorBlock
    ///    & `quality-menu.lua`) while populating real human titles natively via `#EXTM3U` /
    ///    `#EXTINF` `loadlist` AND `file-local-options/force-media-title` across all MPV versions.
    /// 2. Streams are resolved on-demand in pure Rust during MPV's `on_load` hook.
    pub fn spawn(
        runtime: &Runtime,
        engine: Arc<Engine>,
        settings: &AppSettings,
        event_sender: async_channel::Sender<MpvEvent>,
    ) -> Arc<Self> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let pid = std::process::id();
        let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let ipc_dir = PathBuf::from(runtime_dir).join(format!("youtube-rusty-mpv-{pid}-{seq}"));
        let _ = std::fs::create_dir_all(&ipc_dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&ipc_dir, std::fs::Permissions::from_mode(0o700));
        }

        let sock_path = ipc_dir.join("mpv.sock");
        let script_path = ipc_dir.join("yr_bridge.lua");
        let _ = std::fs::write(&script_path, bridge_script(crate::diagnostics::enabled()));

        let initial_quality = Quality::from_setting(&settings.default_quality);
        let default_quality = Arc::new(Mutex::new(initial_quality));

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<QueuedCommand>();
        let (imm_tx, mut imm_rx) = mpsc::unbounded_channel::<String>();
        let process_slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let is_running = Arc::new(AtomicBool::new(true));

        let player_raw = settings.external_player.trim();
        let player_str = if player_raw.is_empty() {
            "mpv"
        } else {
            player_raw
        };
        let mut parts = player_str.split_whitespace();
        let program = parts.next().unwrap_or("mpv").to_string();
        let extra_args: Vec<String> = parts.map(|s| s.to_string()).collect();

        let hwdec_arg = if settings.hardware_decoding {
            "--hwdec=auto-safe"
        } else {
            "--hwdec=no"
        }
        .to_string();

        let ipc_dir_for_task = ipc_dir.clone();
        let proc_for_task = process_slot.clone();
        let running_for_task = is_running.clone();
        let quality_for_task = default_quality.clone();

        let spawn_to_ipc = crate::diagnostics::Span::new("mpv_spawn_to_ipc");
        runtime.spawn(async move {
            let _ = std::fs::remove_file(&sock_path);

            let mut cmd = Command::new(&program);
            for arg in &extra_args {
                cmd.arg(arg);
            }
            cmd.arg("--idle=yes")
                .arg("--force-window=no")
                .arg("--keep-open=no")
                .arg("--ytdl=no")
                .arg(format!("--script={}", script_path.display()))
                .arg(format!("--input-ipc-server={}", sock_path.display()))
                .arg(&hwdec_arg)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    spawn_to_ipc.finish("spawn_error");
                    running_for_task.store(false, Ordering::SeqCst);
                    let _ = std::fs::remove_dir_all(&ipc_dir_for_task);
                    let _ = event_sender
                        .send(MpvEvent::Error(format!(
                            "Could not start '{program}'. Make sure mpv is installed ({e})."
                        )))
                        .await;
                    let _ = event_sender.send(MpvEvent::ProcessExited).await;
                    return;
                }
            };

            let diagnostic_process = crate::diagnostics::MpvProcess::set(child.id());
            crate::diagnostics::milestone("mpv_child_spawned");
            crate::diagnostics::mpv_queue_event("child_spawned", seq, 0, 0);
            if let Ok(mut guard) = proc_for_task.lock() {
                *guard = Some(child);
            }

            // Wait up to 25 seconds for MPV's Unix IPC socket (accommodates cold HDD dynamic linking
            // of FFmpeg/Vulkan/Lua shared libraries while still returning immediately on connect or early exit)
            let mut stream_opt = None;
            for _ in 0..500 {
                if !running_for_task.load(Ordering::SeqCst) {
                    break;
                }
                if sock_path.exists() {
                    if let Ok(s) = UnixStream::connect(&sock_path).await {
                        stream_opt = Some(s);
                        break;
                    }
                }
                let exited_early = proc_for_task
                    .lock()
                    .ok()
                    .and_then(|mut g| g.as_mut().and_then(|c| c.try_wait().ok().flatten()))
                    .is_some();
                if exited_early {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }

            let stream = match stream_opt {
                Some(s) => {
                    spawn_to_ipc.finish("ok");
                    crate::diagnostics::mpv_queue_event("ipc_connected", seq, 0, 0);
                    s
                },
                None => {
                    spawn_to_ipc.finish("ipc_unavailable");
                    running_for_task.store(false, Ordering::SeqCst);
                    if let Ok(mut guard) = proc_for_task.lock() {
                        if let Some(mut c) = guard.take() {
                            let _ = c.kill();
                            let _ = c.wait();
                        }
                    }
                    let _ = std::fs::remove_dir_all(&ipc_dir_for_task);
                    let _ = event_sender
                        .send(MpvEvent::Error(
                            "Could not connect to MPV player.".to_string(),
                        ))
                        .await;
                    let _ = event_sender.send(MpvEvent::ProcessExited).await;
                    return;
                }
            };

            let (reader, mut writer) = stream.into_split();

            let init_cmds = [
                json!({"command": ["observe_property", 1, "time-pos"]}).to_string(),
                json!({"command": ["observe_property", 2, "duration"]}).to_string(),
                json!({"command": ["observe_property", 3, "pause"]}).to_string(),
                json!({"command": ["observe_property", 4, "idle-active"]}).to_string(),
                json!({"command": ["observe_property", 5, "playlist-pos"]}).to_string(),
            ];
            for ic in init_cmds {
                let line = format!("{ic}\n");
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
            let _ = writer.flush().await;

            let bridge_ready = Arc::new(AtomicBool::new(false));
            let bridge_notify = Arc::new(Notify::new());
            let suppress_file_err = Arc::new(AtomicBool::new(false));

            // Writer task: waits until `yr_bridge.lua` has registered its handlers before flushing
            // queued user commands, while allowing immediate internal pings/hook replies at any time.
            let running_write = running_for_task.clone();
            let ready_for_write = bridge_ready.clone();
            let notify_for_write = bridge_notify.clone();
            let imm_tx_ping = imm_tx.clone();
            tokio::spawn(async move {
                let ping_cmd =
                    json!({"command": ["script-message-to", "yr_bridge", "ping"]}).to_string();
                let mut wait_ticks = 0u32;
                while !ready_for_write.load(Ordering::SeqCst) && wait_ticks < 160 {
                    if !running_write.load(Ordering::SeqCst) {
                        return;
                    }
                    let _ = imm_tx_ping.send(ping_cmd.clone());
                    while let Ok(imm) = imm_rx.try_recv() {
                        let line = format!("{imm}\n");
                        if writer.write_all(line.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    let _ = writer.flush().await;
                    tokio::select! {
                        _ = notify_for_write.notified() => break,
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(25)) => {
                            wait_ticks += 1;
                        }
                    }
                }

                loop {
                    if !running_write.load(Ordering::SeqCst) {
                        break;
                    }
                    tokio::select! {
                        biased;
                        Some(imm_str) = imm_rx.recv() => {
                            let line = format!("{imm_str}\n");
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                            let _ = writer.flush().await;
                        }
                        Some(command) = cmd_rx.recv() => {
                            if let Some(trace) = command.trace {
                                crate::diagnostics::mpv_queue_event("write_started", seq, trace.generation, trace.trace_id);
                            }
                            let line = format!("{}\n", command.body);
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                if let Some(trace) = command.trace {
                                    crate::diagnostics::mpv_queue_event("write_error", seq, trace.generation, trace.trace_id);
                                }
                                break;
                            }
                            let flushed = writer.flush().await;
                            if let Some(trace) = command.trace {
                                crate::diagnostics::mpv_queue_event(if flushed.is_ok() { "write_flushed" } else { "flush_error" }, seq, trace.generation, trace.trace_id);
                            }
                        }
                        else => break,
                    }
                }
            });

            // Reader task: forwards MPV state updates to UI and services `yr-resolve` hooks directly
            let running_read = running_for_task.clone();
            let proc_cleanup = proc_for_task.clone();
            let dir_cleanup = ipc_dir_for_task.clone();
            let ev_tx = event_sender.clone();
            let ready_for_read = bridge_ready;
            let notify_for_read = bridge_notify;
            let imm_tx_read = imm_tx;

            tokio::spawn(async move {
                let _diagnostic_process = diagnostic_process;
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !running_read.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };

                    match v.get("event").and_then(|e| e.as_str()) {
                        Some("client-message") => {
                            let Some(args) = v.get("args").and_then(|a| a.as_array()) else {
                                continue;
                            };
                            let tag = args.first().and_then(|s| s.as_str()).unwrap_or("");
                            match tag {
                                "yr-bridge-ready" => {
                                    if !ready_for_read.swap(true, Ordering::SeqCst) {
                                        crate::diagnostics::milestone("mpv_bridge_ready");
                                        crate::diagnostics::mpv_queue_event("bridge_ready", seq, 0, 0);
                                        notify_for_read.notify_waiters();
                                    }
                                }
                                "yr-diagnostics" => {
                                    if crate::diagnostics::enabled() {
                                        if let Some((stage, generation)) = diagnostic_queue_message(args) {
                                            crate::diagnostics::mpv_queue_event(stage, seq, generation, 0);
                                        }
                                    }
                                }
                                "yr-queue-synced" => {
                                    if let Some(gen_num) = args
                                        .get(1)
                                        .and_then(|s| s.as_str())
                                        .and_then(|s| s.parse::<u64>().ok())
                                    {
                                        crate::diagnostics::mpv_queue_event("bridge_queue_synced", seq, gen_num, 0);
                                        let _ = ev_tx.send(MpvEvent::QueueSynced(gen_num)).await;
                                    }
                                }
                                "yr-resolve" => {
                                    let req_id = args
                                        .get(1)
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let url = args
                                        .get(2)
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let ytdl_fmt = args
                                        .get(3)
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if req_id.is_empty() {
                                        continue;
                                    }

                                    let eng = engine.clone();
                                    let ipc_dir_resp = dir_cleanup.clone();
                                    let imm_reply = imm_tx_read.clone();
                                    let ev_resolve = ev_tx.clone();
                                    let sup_err = suppress_file_err.clone();
                                    let def_q = quality_for_task
                                        .lock()
                                        .map(|g| *g)
                                        .unwrap_or(Quality::P360);

                                    tokio::spawn(async move {
                                        let Some(vid_id) = extract_youtube_id(&url) else {
                                            sup_err.store(true, Ordering::SeqCst);
                                            if let Some(resp_path) = write_payload_json(
                                                &ipc_dir_resp,
                                                &format!("resp-{req_id}.json"),
                                                &json!({ "ok": false }),
                                            ) {
                                                let _ = imm_reply.send(
                                                    json!({
                                                        "command": [
                                                            "script-message-to",
                                                            "yr_bridge",
                                                            "resolved",
                                                            req_id,
                                                            resp_path.to_string_lossy()
                                                        ]
                                                    })
                                                    .to_string(),
                                                );
                                            }
                                            return;
                                        };

                                        let fmt_opt = if ytdl_fmt.trim().is_empty() {
                                            None
                                        } else {
                                            Some(ytdl_fmt.as_str())
                                        };

                                        let span = crate::diagnostics::Span::new("mpv_resolve_stream");
                                        let result = eng.resolve_playback_for_mpv(&vid_id, def_q, fmt_opt, span.id()).await;
                                        span.finish(if result.is_ok() { "ok" } else { "error" });
                                        match result {
                                            Ok(resolved) => {
                                                if let Some(d) = resolved.duration_secs {
                                                    let _ = ev_resolve
                                                        .send(MpvEvent::Duration(d as f64))
                                                        .await;
                                                }
                                                let clean_title = if resolved.title.trim().is_empty() {
                                                    String::new()
                                                } else {
                                                    sanitize_m3u_title(&resolved.title, &vid_id)
                                                };
                                                let resp_val = json!({
                                                    "ok": true,
                                                    "title": clean_title,
                                                    "load_url": resolved.to_loadfile_url(),
                                                    "user_agent": resolved.user_agent,
                                                    "referer": resolved.referer,
                                                    "is_audio_only": resolved.is_audio_only,
                                                    "ytdl_json": resolved.ytdl_json,
                                                });
                                                if let Some(resp_path) = write_payload_json(
                                                    &ipc_dir_resp,
                                                    &format!("resp-{req_id}.json"),
                                                    &resp_val,
                                                ) {
                                                    let _ = imm_reply.send(
                                                        json!({
                                                            "command": [
                                                                "script-message-to",
                                                                "yr_bridge",
                                                                "resolved",
                                                                req_id,
                                                                resp_path.to_string_lossy()
                                                            ]
                                                        })
                                                        .to_string(),
                                                    );
                                                }
                                            }
                                            Err(err) => {
                                                sup_err.store(true, Ordering::SeqCst);
                                                let _ = ev_resolve
                                                    .send(MpvEvent::Error(format!(
                                                        "Could not play video: {err}"
                                                    )))
                                                    .await;
                                                if let Some(resp_path) = write_payload_json(
                                                    &ipc_dir_resp,
                                                    &format!("resp-{req_id}.json"),
                                                    &json!({ "ok": false }),
                                                ) {
                                                    let _ = imm_reply.send(
                                                        json!({
                                                            "command": [
                                                                "script-message-to",
                                                                "yr_bridge",
                                                                "resolved",
                                                                req_id,
                                                                resp_path.to_string_lossy()
                                                            ]
                                                        })
                                                        .to_string(),
                                                    );
                                                }
                                            }
                                        }
                                    });
                                }
                                _ => {}
                            }
                        }
                        Some("start-file") => {
                            crate::diagnostics::milestone("mpv_start_file");
                            crate::diagnostics::mpv_queue_event("start_file", seq, 0, 0);
                            let _ = ev_tx.send(MpvEvent::Loading).await;
                        }
                        Some("file-loaded") => {
                            crate::diagnostics::milestone("mpv_file_loaded");
                            crate::diagnostics::mpv_queue_event("file_loaded", seq, 0, 0);
                            let _ = ev_tx.send(MpvEvent::Loaded).await;
                        }
                        Some("end-file") => {
                            let _ = ev_tx.send(MpvEvent::Loaded).await;
                            match v.get("reason").and_then(|r| r.as_str()) {
                                Some("eof") => {
                                    let _ = ev_tx.send(MpvEvent::TrackEnded).await;
                                }
                                Some("error") => {
                                    if !suppress_file_err.swap(false, Ordering::SeqCst) {
                                        let detail = v
                                            .get("file_error")
                                            .and_then(|e| e.as_str())
                                            .unwrap_or("stream unavailable");
                                        let _ = ev_tx
                                            .send(MpvEvent::Error(format!(
                                                "MPV could not play this stream ({detail})"
                                            )))
                                            .await;
                                    }
                                }
                                _ => {}
                            }
                        }
                        Some("property-change") => {
                            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let data = v.get("data");

                            match name {
                                "time-pos" => {
                                    if let Some(num) = data.and_then(|d| d.as_f64()) {
                                        let _ = ev_tx.send(MpvEvent::TimePos(num)).await;
                                    }
                                }
                                "duration" => {
                                    if let Some(num) = data.and_then(|d| d.as_f64()) {
                                        let _ = ev_tx.send(MpvEvent::Duration(num)).await;
                                    }
                                }
                                "pause" => {
                                    if let Some(p) = data.and_then(|d| d.as_bool()) {
                                        let _ = ev_tx.send(MpvEvent::Pause(p)).await;
                                    }
                                }
                                "idle-active" => {
                                    if let Some(idle) = data.and_then(|d| d.as_bool()) {
                                        let _ = ev_tx.send(MpvEvent::Idle(idle)).await;
                                    }
                                }
                                "playlist-pos" => {
                                    if let Some(idx) =
                                        data.and_then(|d| d.as_i64()).filter(|&i| i >= 0)
                                    {
                                        let _ = ev_tx
                                            .send(MpvEvent::PlaylistPos(idx as usize))
                                            .await;
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }

                running_read.store(false, Ordering::SeqCst);
                if let Ok(mut guard) = proc_cleanup.lock() {
                    if let Some(mut c) = guard.take() {
                        let _ = c.try_wait();
                    }
                }
                let _ = std::fs::remove_dir_all(&dir_cleanup);
                let _ = ev_tx.send(MpvEvent::ProcessExited).await;
            });
        });

        Arc::new(Self {
            ipc_dir,
            payload_seq: AtomicU64::new(1),
            default_quality,
            cmd_sender: cmd_tx,
            session_seq: seq,
            process: process_slot,
            is_running,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    pub fn set_default_quality(&self, quality: Quality) {
        if let Ok(mut g) = self.default_quality.lock() {
            *g = quality;
        }
    }

    fn send_json(&self, value: serde_json::Value) {
        let _ = self.cmd_sender.send(QueuedCommand { body: value.to_string(), trace: None });
    }

    fn next_payload_seq(&self) -> u64 {
        self.payload_seq.fetch_add(1, Ordering::Relaxed)
    }

    fn build_title_entries(items: &[MpvQueueItem]) -> Vec<serde_json::Value> {
        items
            .iter()
            .map(|it| {
                json!({
                    "url": youtube_watch_url(&it.video_id),
                    "title": sanitize_m3u_title(&it.title, &it.video_id),
                })
            })
            .collect()
    }

    /// Replaces MPV's playlist atomically via `#EXTM3U` / `#EXTINF` `loadlist` (`0 .. items.len() - 1`),
    /// setting human titles on every playlist entry across all MPV versions and starting playback
    /// directly at `start_index` via `playlist-start`.
    pub fn load_queue(
        &self,
        items: &[MpvQueueItem],
        start_index: usize,
        audio_only: bool,
        sync_gen: u64,
        trace_id: u64,
    ) {
        if items.is_empty() {
            return;
        }
        let prepare = crate::diagnostics::Span::child("mpv_queue_prepare_files", trace_id);
        let n = self.next_payload_seq();
        let Some(m3u_path) = write_m3u8_file(&self.ipc_dir, &format!("load-queue-{n}.m3u8"), items)
        else {
            prepare.finish("m3u_write_error");
            return;
        };
        let entries = Self::build_title_entries(items);
        let payload = json!({
            "items": entries,
            "m3u_path": m3u_path.to_string_lossy(),
            "start_index": start_index.min(items.len() - 1),
            "audio_only": audio_only,
            "sync_gen": sync_gen,
        });
        if let Some(path) = write_payload_json(&self.ipc_dir, &format!("load-queue-{n}.json"), &payload) {
            prepare.finish("ok");
            let body = json!({
                "command": [
                    "script-message-to",
                    "yr_bridge",
                    "load-queue",
                    path.to_string_lossy()
                ]
            }).to_string();
            let trace = crate::diagnostics::enabled().then_some(QueueTrace { generation: sync_gen, trace_id });
            if trace.is_some() {
                crate::diagnostics::mpv_queue_event("enqueue_attempt", self.session_seq, sync_gen, trace_id);
            }
            if self.cmd_sender.send(QueuedCommand { body, trace }).is_err() && trace.is_some() {
                crate::diagnostics::mpv_queue_event("enqueue_error", self.session_seq, sync_gen, trace_id);
            }
        } else {
            prepare.finish("json_write_error");
            let _ = std::fs::remove_file(&m3u_path);
        }
    }

    /// Appends newly loaded items (e.g. background Shorts/playlist continuations or "Add to queue")
    /// to MPV's playlist with real `#EXTINF` human titles without disturbing active playback.
    pub fn append_queue(&self, items: &[MpvQueueItem]) {
        if items.is_empty() {
            return;
        }
        let n = self.next_payload_seq();
        let Some(m3u_path) =
            write_m3u8_file(&self.ipc_dir, &format!("append-queue-{n}.m3u8"), items)
        else {
            return;
        };
        let entries = Self::build_title_entries(items);
        let payload = json!({
            "items": entries,
            "m3u_path": m3u_path.to_string_lossy(),
        });
        if let Some(path) =
            write_payload_json(&self.ipc_dir, &format!("append-queue-{n}.json"), &payload)
        {
            self.send_json(json!({
                "command": [
                    "script-message-to",
                    "yr_bridge",
                    "append-queue",
                    path.to_string_lossy()
                ]
            }));
        } else {
            let _ = std::fs::remove_file(&m3u_path);
        }
    }

    /// Re-synchronizes MPV's playlist around the currently playing item (after reorder, shuffle,
    /// or removal in `PlayerCard`) with real `#EXTINF` human titles without interrupting playback.
    pub fn sync_queue(&self, items: &[MpvQueueItem], current_index: usize, sync_gen: u64) {
        if items.is_empty() {
            return;
        }
        let cur_idx = current_index.min(items.len() - 1);
        let n = self.next_payload_seq();
        let before_slice = &items[..cur_idx];
        let after_slice = &items[cur_idx + 1..];

        let before_m3u =
            write_m3u8_file(&self.ipc_dir, &format!("sync-before-{n}.m3u8"), before_slice);
        let after_m3u =
            write_m3u8_file(&self.ipc_dir, &format!("sync-after-{n}.m3u8"), after_slice);

        let entries = Self::build_title_entries(items);
        let payload = json!({
            "items": entries,
            "before_m3u_path": before_m3u.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "before_count": before_slice.len(),
            "after_m3u_path": after_m3u.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "sync_gen": sync_gen,
        });
        if let Some(path) =
            write_payload_json(&self.ipc_dir, &format!("sync-queue-{n}.json"), &payload)
        {
            self.send_json(json!({
                "command": [
                    "script-message-to",
                    "yr_bridge",
                    "sync-queue",
                    path.to_string_lossy()
                ]
            }));
        } else {
            if let Some(p) = before_m3u {
                let _ = std::fs::remove_file(p);
            }
            if let Some(p) = after_m3u {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    pub fn set_playlist_pos(&self, mpv_index: usize) {
        self.send_json(json!({
            "command": ["set_property", "playlist-pos", mpv_index]
        }));
        self.send_json(json!({
            "command": ["set_property", "pause", false]
        }));
    }

    pub fn set_hardware_decoding(&self, enabled: bool) {
        let mode = if enabled { "auto-safe" } else { "no" };
        self.send_json(json!({ "command": ["set_property", "hwdec", mode] }));
    }

    pub fn toggle_pause(&self) {
        self.send_json(json!({ "command": ["cycle", "pause"] }));
    }

    /// Stops playback, clears MPV's playlist, and hides the MPV window (`force-window=no`)
    /// while keeping the warm `mpv` process alive in the background for instant next playback.
    pub fn stop(&self) {
        self.send_json(json!({ "command": ["set_property", "playlist-start", "auto"] }));
        self.send_json(json!({ "command": ["set_property", "force-window", "no"] }));
        self.send_json(json!({ "command": ["stop"] }));
        self.send_json(json!({ "command": ["playlist-clear"] }));
    }

    pub fn shutdown(&self) {
        self.is_running.store(false, Ordering::SeqCst);
        self.send_json(json!({"command": ["quit"]}));
        if let Ok(mut guard) = self.process.lock() {
            if let Some(mut proc) = guard.take() {
                let _ = proc.kill();
                let _ = proc.wait();
            }
        }
        let _ = std::fs::remove_dir_all(&self.ipc_dir);
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_youtube_id() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/watch?list=PL123&v=abc_DEF-789&index=5")
                .as_deref(),
            Some("abc_DEF-789")
        );
        assert_eq!(
            extract_youtube_id("https://youtu.be/xyz987?t=42").as_deref(),
            Some("xyz987")
        );
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/shorts/shortId_123?feature=share")
                .as_deref(),
            Some("shortId_123")
        );
        assert!(extract_youtube_id("/home/user/Videos/movie.mkv").is_none());
    }

    #[test]
    fn test_m3u8_playlist_titles_and_sanitization() {
        assert_eq!(
            sanitize_m3u_title("Rust, GTK4, and MPV!\r\nPart 2\t🔥", "vid1"),
            "Rust, GTK4, and MPV! Part 2 🔥"
        );
        assert_eq!(sanitize_m3u_title("   \n\t  ", "abc123"), "YouTube (abc123)");

        let queue = vec![
            MpvQueueItem {
                video_id: "short_01".to_string(),
                title: "First Short, With Comma!".to_string(),
            },
            MpvQueueItem {
                video_id: "short_02".to_string(),
                title: "Second\nShort\r\nTitle".to_string(),
            },
            MpvQueueItem {
                video_id: "short_03".to_string(),
                title: String::new(),
            },
        ];
        let m3u = build_m3u8_playlist(&queue);
        let expected = concat!(
            "#EXTM3U\n",
            "#EXTINF:-1,First Short, With Comma!\n",
            "https://www.youtube.com/watch?v=short_01\n",
            "#EXTINF:-1,Second Short Title\n",
            "https://www.youtube.com/watch?v=short_02\n",
            "#EXTINF:-1,YouTube (short_03)\n",
            "https://www.youtube.com/watch?v=short_03\n",
        );
        assert_eq!(m3u, expected);
    }
    #[test]
    fn diagnostics_script_is_gated_and_message_parser_is_whitelisted() {
        let ordinary = bridge_script(false);
        let diagnostic = bridge_script(true);
        assert_eq!(ordinary, YR_BRIDGE_LUA);
        assert!(ordinary.contains("local yr_diagnostics = false"));
        assert!(diagnostic.contains("local yr_diagnostics = true"));
        assert!(diagnostic.contains("diagnostic_queue('queue_received', payload.sync_gen)"));
        assert!(diagnostic.contains("diagnostic_queue('before_loadlist', payload.sync_gen)"));
        assert!(diagnostic.contains("diagnostic_queue('after_loadlist', payload.sync_gen)"));
        assert_eq!(diagnostic_queue_message(&[json!("yr-diagnostics"), json!("queue_received"), json!("42")]), Some(("bridge_queue_received", 42)));
        assert_eq!(diagnostic_queue_message(&[json!("yr-diagnostics"), json!("before_loadlist"), json!("1")]), Some(("bridge_before_loadlist", 1)));
        assert_eq!(diagnostic_queue_message(&[json!("yr-diagnostics"), json!("after_loadlist"), json!("2")]), Some(("bridge_after_loadlist", 2)));
        assert!(diagnostic_queue_message(&[json!("yr-diagnostics"), json!("private user text"), json!("1")]).is_none());
        assert!(diagnostic_queue_message(&[json!("yr-diagnostics"), json!("queue_received"), json!("not-a-number")]).is_none());
        assert!(diagnostic_queue_message(&[json!("yr-diagnostics"), json!("queue_received"), json!("0")]).is_none());
        assert!(diagnostic_queue_message(&[]).is_none());
    }

    // Tests the real bridge over MPV IPC with a generated local silent WAV.
    // No network, video IDs, user configuration, or audio device is needed.
    #[test]
    #[ignore = "requires mpv with Lua support"]
    fn mpv_bridge_local_playback() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream as StdUnixStream;
        use std::time::{Duration, Instant};
        struct Cleanup { child: Child, dir: PathBuf }
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); let _ = std::fs::remove_dir_all(&self.dir); }
        }
        for enabled in [false, true] {
            let dir = std::env::temp_dir().join(format!("yr-mpv-test-{}-{}", std::process::id(), enabled));
            std::fs::create_dir_all(&dir).unwrap();
            let script = dir.join("yr_bridge.lua");
            let socket = dir.join("mpv.sock");
            let wave = dir.join("silence.wav");
            let m3u = dir.join("queue.m3u8");
            let payload = dir.join("queue.json");
            std::fs::write(&script, bridge_script(enabled)).unwrap();
            let data_len = 32000u32; // two seconds of mono 16-bit PCM at 8 kHz
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"RIFF"); bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt "); bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&8000u32.to_le_bytes()); bytes.extend_from_slice(&16000u32.to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes()); bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data"); bytes.extend_from_slice(&data_len.to_le_bytes());
            bytes.resize(44 + data_len as usize, 0);
            std::fs::write(&wave, bytes).unwrap();
            std::fs::write(&m3u, format!("#EXTM3U\n{}\n", wave.display())).unwrap();
            std::fs::write(&payload, json!({
                "items": [{"url": wave.to_string_lossy(), "title": "test"}],
                "m3u_path": m3u.to_string_lossy(), "start_index": 0,
                "audio_only": true, "sync_gen": 42,
            }).to_string()).unwrap();
            let child = Command::new("mpv").args(["--no-config", "--no-terminal", "--idle=yes", "--force-window=no", "--keep-open=no", "--ytdl=no", "--vo=null", "--ao=null"])
                .arg(format!("--script={}", script.display()))
                .arg(format!("--input-ipc-server={}", socket.display()))
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("mpv must be installed");
            let _cleanup = Cleanup { child, dir };
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                if let Ok(stream) = StdUnixStream::connect(&socket) { break stream; }
                assert!(Instant::now() < deadline, "mpv IPC did not start");
                std::thread::sleep(Duration::from_millis(50));
            };
            stream.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let send = |stream: &mut StdUnixStream, value: serde_json::Value| {
                writeln!(stream, "{value}").unwrap();
            };
            let mut ready = false;
            while Instant::now() < deadline && !ready {
                send(&mut stream, json!({"command":["script-message-to", "yr_bridge", "ping"]}));
                let mut line = String::new();
                if reader.read_line(&mut line).is_ok() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                        ready = value.get("args").and_then(|x| x.as_array()).and_then(|x| x.first()).and_then(|x| x.as_str()) == Some("yr-bridge-ready");
                    }
                }
            }
            assert!(ready, "bridge never became ready");
            send(&mut stream, json!({"command":["script-message-to", "yr_bridge", "load-queue", payload.to_string_lossy()]}));
            let mut stages = Vec::new();
            let mut loaded = false;
            let mut synced = false;
            while Instant::now() < deadline && !(loaded && synced && (!enabled || stages.len() == 3)) {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() { continue; }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if value.get("event").and_then(|e| e.as_str()) == Some("file-loaded") { loaded = true; }
                if let Some(args) = value.get("args").and_then(|a| a.as_array()) {
                    if args.first().and_then(|a| a.as_str()) == Some("yr-queue-synced") { synced = true; }
                    if args.first().and_then(|a| a.as_str()) == Some("yr-diagnostics") {
                        stages.push(diagnostic_queue_message(args));
                    }
                }
            }
            assert!(loaded && synced, "local file and queue must load in both modes");
            if enabled {
                assert_eq!(stages, vec![Some(("bridge_queue_received", 42)), Some(("bridge_before_loadlist", 42)), Some(("bridge_after_loadlist", 42))]);
            } else {
                assert!(stages.is_empty(), "diagnostics must remain silent when disabled");
            }
        }
    }

}
