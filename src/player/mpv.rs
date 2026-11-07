use std::process::{Command, Stdio};

pub fn play(video_id: &str, title: &str) -> std::io::Result<()> {
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let title_arg = format!("--force-media-title={title}");

    // Spawn detached child mpv process without blocking the GTK UI
    Command::new("mpv")
        .arg(&title_arg)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
}
