use super::models::{ChannelDetails, VideoSummary};
use rustypipe::client::RustyPipe;
use rustypipe::model::VideoItem;
use rustypipe::param::ChannelVideoTab;

pub struct Engine {
    rp: RustyPipe,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            rp: RustyPipe::new(),
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<VideoSummary>, String> {
        let results = self
            .rp
            .query()
            .search::<VideoItem, _>(query)
            .await
            .map_err(|e| format!("Search error: {e}"))?;

        let videos = results
            .items
            .items
            .into_iter()
            .map(map_video_item)
            .collect();

        Ok(videos)
    }

    pub async fn trending(&self) -> Result<Vec<VideoSummary>, String> {
        let items = self
            .rp
            .query()
            .trending()
            .await
            .map_err(|e| format!("Trending error: {e}"))?;

        let videos = items.into_iter().map(map_video_item).collect();

        Ok(videos)
    }

    pub async fn channel_videos(
        &self,
        channel_id: &str,
    ) -> Result<(ChannelDetails, Vec<VideoSummary>), String> {
        let ch = self
            .rp
            .query()
            .channel_videos(channel_id)
            .await
            .map_err(|e| format!("Channel error: {e}"))?;

        let details = ChannelDetails {
            id: ch.id.clone(),
            name: ch.name.clone(),
            handle: ch
                .handle
                .unwrap_or_else(|| format!("@{}", ch.name.to_lowercase().replace(' ', ""))),
            subscriber_count_text: ch
                .subscriber_count
                .map(format_count)
                .map(|s| format!("{s} subscribers"))
                .unwrap_or_else(|| "Subscribers hidden".to_string()),
            video_count_text: ch
                .video_count
                .map(|v| format!("{v} videos"))
                .unwrap_or_default(),
            avatar_url: ch.avatar.last().map(|t| t.url.clone()).unwrap_or_default(),
        };

        let videos = ch.content.items.into_iter().map(map_video_item).collect();
        Ok((details, videos))
    }

    pub async fn channel_shorts(
        &self,
        channel_id: &str,
    ) -> Result<Vec<VideoSummary>, String> {
        let ch = self
            .rp
            .query()
            .channel_videos_tab(channel_id, ChannelVideoTab::Shorts)
            .await
            .map_err(|e| format!("Channel shorts error: {e}"))?;

        let videos = ch.content.items.into_iter().map(map_video_item).collect();
        Ok(videos)
    }
}

fn map_video_item(item: VideoItem) -> VideoSummary {
    let duration_secs = item.duration.map(|d| d as u64);
    let duration_text = duration_secs
        .map(|d| {
            let mins = d / 60;
            let secs = d % 60;
            format!("{mins}:{secs:02}")
        })
        .unwrap_or_default();

    let view_count_text = item
        .view_count
        .map(format_count)
        .map(|s| format!("{s} views"))
        .unwrap_or_default();

    let thumbnail_url = item
        .thumbnail
        .last()
        .map(|t| t.url.clone())
        .unwrap_or_default();

    let (channel_id, channel_name, channel_avatar_url) = if let Some(c) = item.channel {
        let avatar = c.avatar.last().map(|t| t.url.clone()).unwrap_or_default();
        (c.id, c.name, avatar)
    } else {
        (String::new(), "Unknown".to_string(), String::new())
    };

    let description_snippet = item.short_description.unwrap_or_default();
    let published_text = item.publish_date_txt.unwrap_or_default();

    VideoSummary {
        id: item.id,
        title: item.name,
        channel_id,
        channel_name,
        channel_avatar_url,
        duration_seconds: duration_secs,
        duration_text,
        view_count_text,
        published_text,
        thumbnail_url,
        description_snippet,
    }
}

fn format_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        format!("{count}")
    }
}
