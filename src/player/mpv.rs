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

    pub fn ytdl_format(self) -> Option<&'static str> {
        match self {
            Quality::P360 => Some("bestvideo[height<=360]+bestaudio/best[height<=360]/best"),
            Quality::P720 => Some("bestvideo[height<=720]+bestaudio/best[height<=720]/best"),
            Quality::P1080 => Some("bestvideo[height<=1080]+bestaudio/best[height<=1080]/best"),
            Quality::AudioOnly => None,
        }
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
    }
}
