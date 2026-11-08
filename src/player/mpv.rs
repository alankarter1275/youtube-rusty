use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    P360,
    P720,
    P1080,
    AudioOnly,
}

impl Default for Quality {
    fn default() -> Self {
        Quality::P360
    }
}

pub fn play_with_quality(video_id: &str, title: &str, quality: Quality) -> std::io::Result<()> {
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let title_arg = format!("--force-media-title={title}");

    let mut cmd = Command::new("mpv");
    cmd.arg(&title_arg)
        .arg("--hwdec=auto-safe")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match quality {
        Quality::P360 => {
            cmd.arg("--ytdl-format=bestvideo[height<=360]+bestaudio/best[height<=360]");
        }
        Quality::P720 => {
            cmd.arg("--ytdl-format=bestvideo[height<=720]+bestaudio/best[height<=720]");
        }
        Quality::P1080 => {
            cmd.arg("--ytdl-format=bestvideo[height<=1080]+bestaudio/best[height<=1080]");
        }
        Quality::AudioOnly => {
            cmd.arg("--no-video");
        }
    }

    cmd.arg(&url);
    cmd.spawn()?;

    Ok(())
}

pub fn play(video_id: &str, title: &str) -> std::io::Result<()> {
    play_with_quality(video_id, title, Quality::default())
}
