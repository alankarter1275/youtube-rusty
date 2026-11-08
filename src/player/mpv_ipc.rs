use crate::backend::settings::AppSettings;
use crate::player::mpv::ResolvedPlayback;
use serde_json::json;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum MpvEvent {
    Loading,
    Loaded,
    TimePos(f64),
    Duration(f64),
    Pause(bool),
    TrackEnded,
    Idle(bool),
    Error(String),
    ProcessExited,
}

pub struct MpvSession {
    sock_path: PathBuf,
    cmd_sender: mpsc::UnboundedSender<String>,
    process: Arc<Mutex<Option<Child>>>,
    is_running: Arc<AtomicBool>,
}

impl MpvSession {
    /// Creates an `MpvSession` handle immediately (`--ytdl=no`, zero Python / `yt-dlp`).
    /// Commands sent before MPV finishes opening its Unix socket are queued and flushed in order.
    pub fn spawn(
        runtime: &Runtime,
        settings: &AppSettings,
        event_sender: async_channel::Sender<MpvEvent>,
    ) -> Arc<Self> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let pid = std::process::id();
        let sock_path = PathBuf::from(runtime_dir).join(format!("youtube-rusty-mpv-{pid}.sock"));

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
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

        let sock_for_task = sock_path.clone();
        let proc_for_task = process_slot.clone();
        let running_for_task = is_running.clone();

        runtime.spawn(async move {
            let _ = std::fs::remove_file(&sock_for_task);

            let mut cmd = Command::new(&program);
            for arg in &extra_args {
                cmd.arg(arg);
            }
            cmd.arg("--idle=yes")
                .arg("--force-window=no")
                .arg("--keep-open=no")
                .arg("--ytdl=no")
                .arg(format!("--input-ipc-server={}", sock_for_task.display()))
                .arg(&hwdec_arg)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    running_for_task.store(false, Ordering::SeqCst);
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

            // Wait up to 3 seconds for MPV's Unix IPC socket
            let mut stream_opt = None;
            for _ in 0..60 {
                if !running_for_task.load(Ordering::SeqCst) {
                    break;
                }
                if sock_for_task.exists() {
                    if let Ok(s) = UnixStream::connect(&sock_for_task).await {
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
                    let _ = std::fs::remove_file(&sock_for_task);
                    let _ = event_sender
                        .send(MpvEvent::Error(
                            "Could not connect to MPV player window.".to_string(),
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
            ];
            for ic in init_cmds {
                let line = format!("{ic}\n");
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
            let _ = writer.flush().await;

            // Writer task: drains buffered & future commands in exact order
            let running_write = running_for_task.clone();
            tokio::spawn(async move {
                while let Some(cmd_str) = cmd_rx.recv().await {
                    if !running_write.load(Ordering::SeqCst) {
                        break;
                    }
                    let line = format!("{cmd_str}\n");
                    if writer.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    let _ = writer.flush().await;
                }
            });

            // Reader task: forwards MPV state updates to the UI
            let running_read = running_for_task.clone();
            let proc_cleanup = proc_for_task.clone();
            let sock_cleanup = sock_for_task.clone();
            let ev_tx = event_sender.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !running_read.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        match v.get("event").and_then(|e| e.as_str()) {
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
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                }

                running_read.store(false, Ordering::SeqCst);
                if let Ok(mut guard) = proc_cleanup.lock() {
                    if let Some(mut c) = guard.take() {
                        let _ = c.try_wait();
                    }
                }
                let _ = std::fs::remove_file(&sock_cleanup);
                let _ = ev_tx.send(MpvEvent::ProcessExited).await;
            });
        });

        Arc::new(Self {
            sock_path,
            cmd_sender: cmd_tx,
            process: process_slot,
            is_running,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    fn send_json(&self, value: serde_json::Value) {
        let _ = self.cmd_sender.send(value.to_string());
    }

    pub fn play_resolved(&self, resolved: &ResolvedPlayback, title: &str) {
        if !resolved.user_agent.is_empty() {
            self.send_json(json!({
                "command": ["set_property", "user-agent", resolved.user_agent]
            }));
        }
        self.send_json(json!({
            "command": ["set_property", "referrer", "https://www.youtube.com/"]
        }));
        if resolved.is_audio_only {
            self.send_json(json!({ "command": ["set_property", "vid", "no"] }));
            self.send_json(json!({ "command": ["set_property", "force-window", "no"] }));
        } else {
            self.send_json(json!({ "command": ["set_property", "vid", "auto"] }));
            self.send_json(json!({ "command": ["set_property", "force-window", "immediate"] }));
        }
        self.send_json(json!({ "command": ["set_property", "pause", false] }));
        self.send_json(json!({ "command": ["set_property", "force-media-title", title] }));

        let load_url = resolved.to_loadfile_url();
        self.send_json(json!({ "command": ["loadfile", load_url, "replace"] }));
    }

    pub fn set_hardware_decoding(&self, enabled: bool) {
        let mode = if enabled { "auto-safe" } else { "no" };
        self.send_json(json!({ "command": ["set_property", "hwdec", mode] }));
    }

    pub fn toggle_pause(&self) {
        self.send_json(json!({ "command": ["cycle", "pause"] }));
    }

    pub fn stop(&self) {
        self.send_json(json!({ "command": ["stop"] }));
        self.send_json(json!({ "command": ["set_property", "force-window", "no"] }));
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
        let _ = std::fs::remove_file(&self.sock_path);
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
