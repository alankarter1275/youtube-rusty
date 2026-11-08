use crate::player::mpv::Quality;
use serde_json::json;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum MpvEvent {
    TimePos(f64),
    Duration(f64),
    Pause(bool),
    PlaylistPos(usize),
    Idle(bool),
    ProcessExited,
}

pub struct MpvSession {
    sock_path: PathBuf,
    cmd_sender: mpsc::UnboundedSender<String>,
    process: Option<Child>,
    is_running: Arc<AtomicBool>,
}

impl MpvSession {
    pub async fn start(
        event_sender: async_channel::Sender<MpvEvent>,
    ) -> Result<Arc<Self>, String> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| "/tmp".into());
        let sock_path = PathBuf::from(runtime_dir).join("youtube-rusty-mpv.sock");

        // Clean up any stale socket
        let _ = std::fs::remove_file(&sock_path);

        // Spawn mpv process
        let process = Command::new("mpv")
            .arg("--idle=yes")
            .arg("--force-window=yes")
            .arg(format!("--input-ipc-server={}", sock_path.display()))
            .arg("--hwdec=auto-safe")
            .arg("--ytdl-format=bestvideo[height<=360]+bestaudio/best[height<=360]")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn mpv: {e}"))?;

        // Wait for socket to appear with retry
        let mut stream_opt = None;
        for _ in 0..40 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if sock_path.exists() {
                if let Ok(s) = UnixStream::connect(&sock_path).await {
                    stream_opt = Some(s);
                    break;
                }
            }
        }

        let stream = match stream_opt {
            Some(s) => s,
            None => {
                return Err("Failed to connect to mpv socket in time".to_string());
            }
        };

        let (reader, mut writer) = stream.into_split();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();

        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_write = is_running.clone();
        let is_running_read = is_running.clone();

        // Writer task
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                if !is_running_write.load(Ordering::SeqCst) {
                    break;
                }
                let line = format!("{cmd}\n");
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        });

        // Reader task
        let ev_tx = event_sender.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !is_running_read.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(event) = v.get("event").and_then(|e| e.as_str()) {
                        if event == "property-change" {
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
                                "playlist-pos" => {
                                    if let Some(idx) = data.and_then(|d| d.as_u64()) {
                                        let _ = ev_tx.send(MpvEvent::PlaylistPos(idx as usize)).await;
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
                    }
                }
            }
            let _ = ev_tx.send(MpvEvent::ProcessExited).await;
        });

        // Send observe_property commands
        let _ = cmd_tx.send(json!({"command": ["observe_property", 1, "time-pos"]}).to_string());
        let _ = cmd_tx.send(json!({"command": ["observe_property", 2, "duration"]}).to_string());
        let _ = cmd_tx.send(json!({"command": ["observe_property", 3, "pause"]}).to_string());
        let _ = cmd_tx.send(json!({"command": ["observe_property", 4, "playlist-pos"]}).to_string());
        let _ = cmd_tx.send(json!({"command": ["observe_property", 5, "idle-active"]}).to_string());

        Ok(Arc::new(Self {
            sock_path,
            cmd_sender: cmd_tx,
            process: Some(process),
            is_running,
        }))
    }

    pub fn play(&self, video_id: &str, title: &str, quality: Quality) {
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        self.set_quality(quality);
        let cmd = json!({
            "command": ["loadfile", url, "replace"]
        });
        let _ = self.cmd_sender.send(cmd.to_string());
        let _ = self.cmd_sender.send(json!({
            "command": ["set_property", "force-media-title", title]
        }).to_string());
    }

    pub fn append(&self, video_id: &str, title: &str) {
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        let cmd = json!({
            "command": ["loadfile", url, "append"]
        });
        let _ = self.cmd_sender.send(cmd.to_string());
        let _ = self.cmd_sender.send(json!({
            "command": ["set_property", "force-media-title", title]
        }).to_string());
    }

    pub fn set_quality(&self, quality: Quality) {
        match quality {
            Quality::P360 => {
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "ytdl-format", "bestvideo[height<=360]+bestaudio/best[height<=360]"]
                }).to_string());
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "vid", "auto"]
                }).to_string());
            }
            Quality::P720 => {
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "ytdl-format", "bestvideo[height<=720]+bestaudio/best[height<=720]"]
                }).to_string());
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "vid", "auto"]
                }).to_string());
            }
            Quality::P1080 => {
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "ytdl-format", "bestvideo[height<=1080]+bestaudio/best[height<=1080]"]
                }).to_string());
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "vid", "auto"]
                }).to_string());
            }
            Quality::AudioOnly => {
                let _ = self.cmd_sender.send(json!({
                    "command": ["set_property", "vid", "no"]
                }).to_string());
            }
        }
    }

    pub fn toggle_pause(&self) {
        let _ = self.cmd_sender.send(json!({
            "command": ["cycle", "pause"]
        }).to_string());
    }

    pub fn next(&self) {
        let _ = self.cmd_sender.send(json!({
            "command": ["playlist-next"]
        }).to_string());
    }

    pub fn prev(&self) {
        let _ = self.cmd_sender.send(json!({
            "command": ["playlist-prev"]
        }).to_string());
    }

    pub fn jump_to(&self, index: usize) {
        let _ = self.cmd_sender.send(json!({
            "command": ["set_property", "playlist-pos", index]
        }).to_string());
    }

    pub fn remove_at(&self, index: usize) {
        let _ = self.cmd_sender.send(json!({
            "command": ["playlist-remove", index]
        }).to_string());
    }

    pub fn clear(&self) {
        let _ = self.cmd_sender.send(json!({
            "command": ["playlist-clear"]
        }).to_string());
    }

    pub fn stop(&self) {
        let _ = self.cmd_sender.send(json!({
            "command": ["stop"]
        }).to_string());
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
        let _ = self.cmd_sender.send(json!({"command": ["quit"]}).to_string());
        if let Some(mut proc) = self.process.take() {
            let _ = proc.kill();
        }
        let _ = std::fs::remove_file(&self.sock_path);
    }
}
