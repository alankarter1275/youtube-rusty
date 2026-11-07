use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSummary {
    pub id: String,
    pub title: String,
    pub channel_name: String,
    pub duration_seconds: Option<u64>,
    pub duration_text: String,
    pub view_count_text: String,
    pub thumbnail_url: String,
    pub description_snippet: String,
}
