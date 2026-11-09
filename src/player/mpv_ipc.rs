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

local req_counter = 0
local active_req_id = nil
local pending_hooks = {}

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

mp.register_script_message('ping', function()
    mp.commandv('script-message', 'yr-bridge-ready')
end)

mp.register_script_message('load-queue', function(json_path)
    local payload = read_and_remove_json(json_path)
    if not payload or not payload.items or #payload.items == 0 then return end
    local items = payload.items
    local start_idx = tonumber(payload.start_index) or 0
    if start_idx < 0 then start_idx = 0 end
    if start_idx >= #items then start_idx = #items - 1 end

    mp.set_property('force-window', payload.audio_only and 'no' or 'yes')
    mp.set_property_bool('pause', false)

    local start_item = items[start_idx + 1]
    mp.commandv('loadfile', start_item.url, 'replace')

    for k = 0, start_idx - 1 do
        mp.commandv('loadfile', items[k + 1].url, 'append')
        mp.commandv('playlist-move', tostring(k + 1), tostring(k))
    end

    for k = start_idx + 1, #items - 1 do
        mp.commandv('loadfile', items[k + 1].url, 'append')
    end

    for i = 0, #items - 1 do
        local t = items[i + 1].title
        if t and t ~= '' then
            mp.set_property('playlist/' .. tostring(i) .. '/title', t)
        end
    end

    if payload.sync_gen then
        mp.commandv('script-message', 'yr-queue-synced', tostring(payload.sync_gen))
    end
end)

mp.register_script_message('append-queue', function(json_path)
    local payload = read_and_remove_json(json_path)
    if not payload or not payload.items or #payload.items == 0 then return end
    local items = payload.items
    local base_idx = mp.get_property_number('playlist-count', 0)

    for i = 1, #items do
        local it = items[i]
        mp.commandv('loadfile', it.url, 'append')
        if it.title and it.title ~= '' then
            mp.set_property('playlist/' .. tostring(base_idx + i - 1) .. '/title', it.title)
        end
    end
end)

mp.register_script_message('sync-queue', function(json_path)
    local payload = read_and_remove_json(json_path)
    if not payload or not payload.items or #payload.items == 0 then return end
    local items = payload.items
    local cur_idx = tonumber(payload.current_index) or 0
    if cur_idx < 0 then cur_idx = 0 end
    if cur_idx >= #items then cur_idx = #items - 1 end

    mp.command('playlist-clear')

    for k = 0, cur_idx - 1 do
        mp.commandv('loadfile', items[k + 1].url, 'append')
        mp.commandv('playlist-move', tostring(k + 1), tostring(k))
    end

    for k = cur_idx + 1, #items - 1 do
        mp.commandv('loadfile', items[k + 1].url, 'append')
    end

    for i = 0, #items - 1 do
        local t = items[i + 1].title
        if t and t ~= '' then
            mp.set_property('playlist/' .. tostring(i) .. '/title', t)
        end
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
                local media_title = (pl_title and pl_title ~= '') and pl_title or data.title
                if media_title and media_title ~= '' then
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
    local path = mp.get_property('stream-open-filename', '')
    if not is_youtube_url(path) then return end

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

mp.commandv('script-message', 'yr-bridge-ready')
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

pub struct MpvSession {
    ipc_dir: PathBuf,
    payload_seq: AtomicU64,
    default_quality: Arc<Mutex<Quality>>,
    cmd_sender: mpsc::UnboundedSender<String>,
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
    ///    & `quality-menu.lua`) while displaying its real human title via `M_PROPERTY_SET`.
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
        let _ = std::fs::write(&script_path, YR_BRIDGE_LUA);

        let initial_quality = Quality::from_setting(&settings.default_quality);
        let default_quality = Arc::new(Mutex::new(initial_quality));

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
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

            if let Ok(mut guard) = proc_for_task.lock() {
                *guard = Some(child);
            }

            // Wait up to 5 seconds for MPV's Unix IPC socket
            let mut stream_opt = None;
            for _ in 0..100 {
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
                Some(s) => s,
                None => {
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
                while !ready_for_write.load(Ordering::SeqCst) && wait_ticks < 80 {
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
                        Some(cmd_str) = cmd_rx.recv() => {
                            let line = format!("{cmd_str}\n");
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                            let _ = writer.flush().await;
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
                                        notify_for_read.notify_waiters();
                                    }
                                }
                                "yr-queue-synced" => {
                                    if let Some(gen_num) = args
                                        .get(1)
                                        .and_then(|s| s.as_str())
                                        .and_then(|s| s.parse::<u64>().ok())
                                    {
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

                                        match eng
                                            .resolve_playback_for_mpv(&vid_id, def_q, fmt_opt)
                                            .await
                                        {
                                            Ok(resolved) => {
                                                if let Some(d) = resolved.duration_secs {
                                                    let _ = ev_resolve
                                                        .send(MpvEvent::Duration(d as f64))
                                                        .await;
                                                }
                                                let resp_val = json!({
                                                    "ok": true,
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
                            let _ = ev_tx.send(MpvEvent::Loading).await;
                        }
                        Some("file-loaded") => {
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
        let _ = self.cmd_sender.send(value.to_string());
    }

    fn next_payload_filename(&self, prefix: &str) -> String {
        let n = self.payload_seq.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{n}.json")
    }

    /// Replaces MPV's playlist with the full `items` list (`0 .. items.len() - 1`),
    /// preserving all items before `start_index` via `playlist-move`, setting human titles
    /// on every playlist entry, and starting playback immediately at `start_index`.
    pub fn load_queue(
        &self,
        items: &[MpvQueueItem],
        start_index: usize,
        audio_only: bool,
        sync_gen: u64,
    ) {
        if items.is_empty() {
            return;
        }
        let entries: Vec<serde_json::Value> = items
            .iter()
            .map(|it| {
                json!({
                    "url": youtube_watch_url(&it.video_id),
                    "title": it.title,
                })
            })
            .collect();
        let payload = json!({
            "items": entries,
            "start_index": start_index.min(items.len() - 1),
            "audio_only": audio_only,
            "sync_gen": sync_gen,
        });
        let fname = self.next_payload_filename("load-queue");
        if let Some(path) = write_payload_json(&self.ipc_dir, &fname, &payload) {
            self.send_json(json!({
                "command": [
                    "script-message-to",
                    "yr_bridge",
                    "load-queue",
                    path.to_string_lossy()
                ]
            }));
        }
    }

    /// Appends newly loaded items (e.g. background Shorts/playlist continuations or "Add to queue")
    /// to MPV's playlist with real human titles without disturbing active playback.
    pub fn append_queue(&self, items: &[MpvQueueItem]) {
        if items.is_empty() {
            return;
        }
        let entries: Vec<serde_json::Value> = items
            .iter()
            .map(|it| {
                json!({
                    "url": youtube_watch_url(&it.video_id),
                    "title": it.title,
                })
            })
            .collect();
        let payload = json!({ "items": entries });
        let fname = self.next_payload_filename("append-queue");
        if let Some(path) = write_payload_json(&self.ipc_dir, &fname, &payload) {
            self.send_json(json!({
                "command": [
                    "script-message-to",
                    "yr_bridge",
                    "append-queue",
                    path.to_string_lossy()
                ]
            }));
        }
    }

    /// Re-synchronizes MPV's playlist around the currently playing item (after reorder, shuffle,
    /// or removal in `PlayerCard`) without interrupting playback.
    pub fn sync_queue(&self, items: &[MpvQueueItem], current_index: usize, sync_gen: u64) {
        if items.is_empty() {
            return;
        }
        let entries: Vec<serde_json::Value> = items
            .iter()
            .map(|it| {
                json!({
                    "url": youtube_watch_url(&it.video_id),
                    "title": it.title,
                })
            })
            .collect();
        let payload = json!({
            "items": entries,
            "current_index": current_index.min(items.len() - 1),
            "sync_gen": sync_gen,
        });
        let fname = self.next_payload_filename("sync-queue");
        if let Some(path) = write_payload_json(&self.ipc_dir, &fname, &payload) {
            self.send_json(json!({
                "command": [
                    "script-message-to",
                    "yr_bridge",
                    "sync-queue",
                    path.to_string_lossy()
                ]
            }));
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
        self.send_json(json!({ "command": ["set_property", "force-window", "no"] }));
        self.send_json(json!({ "command": ["stop"] }));
        self.send_json(json!({ "command": ["playlist-clear"] }));
    }

    pub fn shutdown(&self) {
        self.is_running.store(false, Ordering::SeqCst);
        let _ = self.cmd_sender.send(json!({"command": ["quit"]}).to_string());
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
}
