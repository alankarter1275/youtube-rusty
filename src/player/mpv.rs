#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    #[default]
    P360,
    P720,
    P1080,
    AudioOnly,
}

impl Quality {
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "720p" => Quality::P720,
            "1080p" => Quality::P1080,
            "audio" | "audio only" | "audio_only" => Quality::AudioOnly,
            _ => Quality::P360,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Quality::P360 => "360p",
            Quality::P720 => "720p",
            Quality::P1080 => "1080p",
            Quality::AudioOnly => "audio",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::P360 => "360p",
            Quality::P720 => "720p",
            Quality::P1080 => "1080p",
            Quality::AudioOnly => "Audio",
        }
    }

    pub fn target_height(self) -> Option<u32> {
        match self {
            Quality::P360 => Some(360),
            Quality::P720 => Some(720),
            Quality::P1080 => Some(1080),
            Quality::AudioOnly => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedPlayback {
    pub primary_url: String,
    pub audio_url: Option<String>,
    pub user_agent: String,
    pub is_audio_only: bool,
    pub duration_secs: Option<u64>,
}

impl ResolvedPlayback {
    /// Builds either a direct stream URL (for muxed MP4, HLS manifests, or audio-only)
    /// or MPV's native multi-track `edl://` timeline URL when separate adaptive video + audio
    /// streams are combined (works across all MPV versions with `--ytdl=no`).
    pub fn to_loadfile_url(&self) -> String {
        if let Some(ref aud) = self.audio_url {
            if !aud.is_empty() && !self.is_audio_only {
                return format!(
                    "edl://!new_stream;!no_clip;!no_chapters;%{}%{};!new_stream;!no_clip;!no_chapters;%{}%{}",
                    self.primary_url.len(),
                    self.primary_url,
                    aud.len(),
                    aud,
                );
            }
        }
        self.primary_url.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quality_parsing() {
        assert_eq!(Quality::from_setting("360p"), Quality::P360);
        assert_eq!(Quality::from_setting("720p"), Quality::P720);
        assert_eq!(Quality::from_setting("1080p"), Quality::P1080);
        assert_eq!(Quality::from_setting("audio"), Quality::AudioOnly);
        assert_eq!(Quality::from_setting("Audio only"), Quality::AudioOnly);
        assert_eq!(Quality::from_setting("unknown"), Quality::P360);
        assert_eq!(Quality::P720.target_height(), Some(720));
        assert_eq!(Quality::AudioOnly.target_height(), None);
    }

    #[test]
    fn test_edl_loadfile_url() {
        let muxed = ResolvedPlayback {
            primary_url: "https://example.com/v.mp4".to_string(),
            audio_url: None,
            user_agent: "UA".to_string(),
            is_audio_only: false,
            duration_secs: Some(60),
        };
        assert_eq!(muxed.to_loadfile_url(), "https://example.com/v.mp4");

        let adaptive = ResolvedPlayback {
            primary_url: "https://example.com/video".to_string(),
            audio_url: Some("https://example.com/audio".to_string()),
            user_agent: "UA".to_string(),
            is_audio_only: false,
            duration_secs: Some(60),
        };
        assert_eq!(
            adaptive.to_loadfile_url(),
            "edl://!new_stream;!no_clip;!no_chapters;%25%https://example.com/video;!new_stream;!no_clip;!no_chapters;%25%https://example.com/audio"
        );
    }
}
