use super::models::VideoSummary;
use rustypipe::client::RustyPipe;
use rustypipe::model::VideoItem;

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
            .map(|item| {
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
                    .map(|v| {
                        if v >= 1_000_000 {
                            format!("{:.1}M views", v as f64 / 1_000_000.0)
                        } else if v >= 1_000 {
                            format!("{:.1}K views", v as f64 / 1_000.0)
                        } else {
                            format!("{v} views")
                        }
                    })
                    .unwrap_or_default();

                let thumbnail_url = item
                    .thumbnail
                    .last()
                    .map(|t| t.url.clone())
                    .unwrap_or_default();

                let channel_name = item
                    .channel
                    .map(|c| c.name)
                    .unwrap_or_else(|| "Unknown".to_string());

                VideoSummary {
                    id: item.id,
                    title: item.name,
                    channel_name,
                    duration_seconds: duration_secs,
                    duration_text,
                    view_count_text,
                    thumbnail_url,
                }
            })
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

        let videos = items
            .into_iter()
            .map(|item| {
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
                    .map(|v| {
                        if v >= 1_000_000 {
                            format!("{:.1}M views", v as f64 / 1_000_000.0)
                        } else if v >= 1_000 {
                            format!("{:.1}K views", v as f64 / 1_000.0)
                        } else {
                            format!("{v} views")
                        }
                    })
                    .unwrap_or_default();

                let thumbnail_url = item
                    .thumbnail
                    .last()
                    .map(|t| t.url.clone())
                    .unwrap_or_default();

                let channel_name = item
                    .channel
                    .map(|c| c.name)
                    .unwrap_or_else(|| "Unknown".to_string());

                VideoSummary {
                    id: item.id,
                    title: item.name,
                    channel_name,
                    duration_seconds: duration_secs,
                    duration_text,
                    view_count_text,
                    thumbnail_url,
                }
            })
            .collect();

        Ok(videos)
    }
}
