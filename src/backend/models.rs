use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VideoSummary {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_avatar_url: String,
    pub duration_seconds: Option<u64>,
    pub duration_text: String,
    pub view_count_text: String,
    #[serde(default)]
    pub view_count: Option<u64>,
    pub published_text: String,
    #[serde(default)]
    pub published_unix: Option<i64>,
    #[serde(default)]
    pub is_short: bool,
    pub thumbnail_url: String,
    #[serde(default)]
    pub description_snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelDetails {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub subscriber_count_text: String,
    pub video_count_text: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelSearchSummary {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub subscriber_count_text: String,
    pub description: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistSearchSummary {
    pub id: String,
    pub name: String,
    pub channel_name: String,
    pub video_count_text: String,
    pub thumbnail_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchItem {
    Video(VideoSummary),
    Channel(ChannelSearchSummary),
    Playlist(PlaylistSearchSummary),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchTab {
    #[default]
    All,
    Videos,
    Shorts,
    Channels,
    Playlists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UploadDateFilter {
    #[default]
    AnyTime,
    Today,
    ThisWeek,
    ThisMonth,
    ThisYear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DurationFilter {
    #[default]
    AnyLength,
    Under4Min,
    FourTo20Min,
    Over20Min,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelSortOrder {
    #[default]
    Newest,
    Oldest,
    Popular,
}

impl ChannelSortOrder {
    pub fn button_label(self) -> &'static str {
        match self {
            ChannelSortOrder::Newest => "Sort: Newest",
            ChannelSortOrder::Oldest => "Sort: Oldest",
            ChannelSortOrder::Popular => "Sort: Popular",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchSortOrder {
    #[default]
    Relevance,
    UploadDate,
    ViewCount,
}

pub fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let rem_secs = secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{rem_secs:02}")
    } else {
        format!("{mins}:{rem_secs:02}")
    }
}

pub fn format_count(count: u64) -> String {
    if count >= 1_000_000_000 {
        format!("{:.1}B", count as f64 / 1_000_000_000.0)
    } else if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        format!("{count}")
    }
}

pub fn format_queue_total_duration(total_secs: u64) -> String {
    if total_secs == 0 {
        return String::new();
    }
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    if hours > 0 && mins > 0 {
        format!("{hours} h {mins} min")
    } else if hours > 0 {
        format!("{hours} h")
    } else if mins > 0 {
        format!("{mins} min")
    } else {
        format!("{total_secs} sec")
    }
}

pub fn looks_like_youtube_url(input: &str) -> bool {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    let stripped = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    stripped.starts_with("youtube.com/")
        || stripped.starts_with("www.youtube.com/")
        || stripped.starts_with("m.youtube.com/")
        || stripped.starts_with("music.youtube.com/")
        || stripped.starts_with("youtu.be/")
}

pub fn matches_local_filters(
    video: &VideoSummary,
    date_filter: UploadDateFilter,
    dur_filter: DurationFilter,
) -> bool {
    let dur_ok = match dur_filter {
        DurationFilter::AnyLength => true,
        DurationFilter::Under4Min => video.duration_seconds.map(|d| d < 240).unwrap_or(false),
        DurationFilter::FourTo20Min => video
            .duration_seconds
            .map(|d| (240..=1200).contains(&d))
            .unwrap_or(false),
        DurationFilter::Over20Min => video.duration_seconds.map(|d| d > 1200).unwrap_or(false),
    };
    if !dur_ok {
        return false;
    }

    match date_filter {
        UploadDateFilter::AnyTime => true,
        _ => {
            let max_age_secs: i64 = match date_filter {
                UploadDateFilter::AnyTime => return true,
                UploadDateFilter::Today => 86_400,
                UploadDateFilter::ThisWeek => 7 * 86_400,
                UploadDateFilter::ThisMonth => 31 * 86_400,
                UploadDateFilter::ThisYear => 366 * 86_400,
            };

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            if let Some(ts) = video.published_unix {
                now.saturating_sub(ts) <= max_age_secs
            } else {
                matches_published_text_heuristic(&video.published_text, date_filter)
            }
        }
    }
}

fn matches_published_text_heuristic(text: &str, filter: UploadDateFilter) -> bool {
    let lower = text.to_lowercase();
    if lower.is_empty() {
        // A restricted date filter must not silently admit items whose age is unknown.
        return false;
    }
    let is_today = lower.contains("second")
        || lower.contains("minute")
        || lower.contains("hour")
        || lower.contains("just now")
        || lower.contains("today");
    let is_week = is_today
        || (lower.contains("day")
            && lower
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .map(|d| d <= 7)
                .unwrap_or(true));
    let is_month = is_week || lower.contains("day") || lower.contains("week");
    let is_year = is_month || lower.contains("month");

    match filter {
        UploadDateFilter::AnyTime => true,
        UploadDateFilter::Today => is_today,
        UploadDateFilter::ThisWeek => is_week,
        UploadDateFilter::ThisMonth => is_month,
        UploadDateFilter::ThisYear => is_year,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(58), "0:58");
        assert_eq!(format_duration(742), "12:22");
        assert_eq!(format_duration(3728), "1:02:08");
    }

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(1_500), "1.5K");
        assert_eq!(format_count(19_800_000), "19.8M");
        assert_eq!(format_count(2_300_000_000), "2.3B");
    }

    #[test]
    fn test_format_queue_total_duration() {
        assert_eq!(format_queue_total_duration(0), "");
        assert_eq!(format_queue_total_duration(45), "45 sec");
        assert_eq!(format_queue_total_duration(6120), "1 h 42 min");
    }

    #[test]
    fn test_channel_filters_exclude_unknown_dates_and_match_duration() {
        let video = VideoSummary {
            id: "test".to_string(),
            title: "test".to_string(),
            channel_id: String::new(),
            channel_name: String::new(),
            channel_avatar_url: String::new(),
            duration_seconds: Some(180),
            duration_text: "3:00".to_string(),
            view_count_text: String::new(),
            view_count: None,
            published_text: String::new(),
            published_unix: None,
            is_short: false,
            thumbnail_url: String::new(),
            description_snippet: String::new(),
        };
        assert!(matches_local_filters(
            &video,
            UploadDateFilter::AnyTime,
            DurationFilter::Under4Min
        ));
        assert!(!matches_local_filters(
            &video,
            UploadDateFilter::Today,
            DurationFilter::AnyLength
        ));
    }

    #[test]
    fn test_youtube_url_detection() {
        assert!(looks_like_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(looks_like_youtube_url("https://youtu.be/dQw4w9WgXcQ"));
        assert!(looks_like_youtube_url("youtube.com/playlist?list=PL123"));
        assert!(!looks_like_youtube_url("rust gtk4 tutorial"));
    }
}
