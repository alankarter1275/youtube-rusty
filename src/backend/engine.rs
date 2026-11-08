use super::models::{
    format_count, format_duration, ChannelDetails, ChannelSortOrder, DurationFilter,
    SearchSortOrder, UploadDateFilter, VideoSummary,
};
use super::subscriptions::SubscribedChannel;
use crate::player::mpv::Quality;
use rustypipe::client::RustyPipe;
use rustypipe::model::{Thumbnail, VideoItem};
use rustypipe::param::search_filter::{ItemType, Length, Order, SearchFilter, UploadDate};
use rustypipe::param::{ChannelOrder, ChannelVideoTab};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub struct Engine {
    rp: RustyPipe,
}

impl Engine {
    pub fn new(storage_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(storage_dir);
        let rp = RustyPipe::builder()
            .storage_dir(storage_dir)
            .build()
            .unwrap_or_else(|_| RustyPipe::new());
        Self { rp }
    }

    pub async fn search_filtered(
        &self,
        query: &str,
        date: UploadDateFilter,
        duration: DurationFilter,
        sort: SearchSortOrder,
    ) -> Result<Vec<VideoSummary>, String> {
        let is_default = date == UploadDateFilter::AnyTime
            && duration == DurationFilter::AnyLength
            && sort == SearchSortOrder::Relevance;

        let items = if is_default {
            self.rp
                .query()
                .search::<VideoItem, _>(query)
                .await
                .map_err(|e| format!("Search error: {e}"))?
                .items
                .items
        } else {
            let rp_date = match date {
                UploadDateFilter::AnyTime => None,
                UploadDateFilter::Today => Some(UploadDate::Day),
                UploadDateFilter::ThisWeek => Some(UploadDate::Week),
                UploadDateFilter::ThisMonth => Some(UploadDate::Month),
                UploadDateFilter::ThisYear => Some(UploadDate::Year),
            };
            let rp_len = match duration {
                DurationFilter::AnyLength => None,
                DurationFilter::Under4Min => Some(Length::Short),
                DurationFilter::FourTo20Min => Some(Length::Medium),
                DurationFilter::Over20Min => Some(Length::Long),
            };
            let rp_sort = match sort {
                SearchSortOrder::Relevance => None,
                SearchSortOrder::UploadDate => Some(Order::Date),
                SearchSortOrder::ViewCount => Some(Order::Views),
            };

            let filter = SearchFilter::new()
                .item_type(ItemType::Video)
                .date_opt(rp_date)
                .length_opt(rp_len)
                .sort_opt(rp_sort);

            self.rp
                .query()
                .search_filter::<VideoItem, _>(query, &filter)
                .await
                .map_err(|e| format!("Search error: {e}"))?
                .items
                .items
        };

        Ok(items
            .into_iter()
            .map(|it| map_video_item(it, None, false))
            .collect())
    }

    pub async fn category_feed(&self, category: &str) -> Result<Vec<VideoSummary>, String> {
        let filter = SearchFilter::new()
            .item_type(ItemType::Video)
            .date(UploadDate::Week);

        let items = match self
            .rp
            .query()
            .search_filter::<VideoItem, _>(category, &filter)
            .await
        {
            Ok(res) if !res.items.items.is_empty() => res.items.items,
            _ => {
                self.rp
                    .query()
                    .search::<VideoItem, _>(category)
                    .await
                    .map_err(|e| format!("Category error: {e}"))?
                    .items
                    .items
            }
        };

        Ok(items
            .into_iter()
            .filter(|it| !it.is_short)
            .map(|it| map_video_item(it, None, false))
            .collect())
    }

    pub async fn trending(&self) -> Result<Vec<VideoSummary>, String> {
        // YouTube retired the legacy FEtrending browse endpoint (HTTP 400).
        // Build a concurrent discovery feed from popular full-length uploads this week.
        let topics = ["music", "gaming", "technology", "news"];
        let mut join_set = JoinSet::new();

        for (idx, topic) in topics.into_iter().enumerate() {
            let rp = self.rp.clone();
            join_set.spawn(async move {
                let filter = SearchFilter::new()
                    .item_type(ItemType::Video)
                    .date(UploadDate::Week);
                let res = rp
                    .query()
                    .search_filter::<VideoItem, _>(topic, &filter)
                    .await
                    .map(|r| r.items.items)
                    .map_err(|e| e.to_string());
                (idx, res)
            });
        }

        let mut buckets: Vec<Vec<VideoSummary>> = vec![Vec::new(); topics.len()];
        let mut last_err = None;

        while let Some(joined) = join_set.join_next().await {
            if let Ok((idx, res)) = joined {
                match res {
                    Ok(items) => {
                        buckets[idx] = items
                            .into_iter()
                            .filter(|it| !it.is_short && it.duration.unwrap_or(120) >= 60)
                            .map(|it| map_video_item(it, None, false))
                            .collect();
                    }
                    Err(err) => last_err = Some(err),
                }
            }
        }

        // Interleave round-robin across topics and deduplicate by video ID
        let mut merged = Vec::new();
        let mut seen = HashSet::new();
        let max_len = buckets.iter().map(|b| b.len()).max().unwrap_or(0);
        for i in 0..max_len {
            for bucket in &buckets {
                if let Some(v) = bucket.get(i) {
                    if seen.insert(v.id.clone()) {
                        merged.push(v.clone());
                        if merged.len() >= 28 {
                            return Ok(merged);
                        }
                    }
                }
            }
        }

        if !merged.is_empty() {
            return Ok(merged);
        }

        // Final fallback to general search if filtered queries returned nothing
        let fallback = self
            .rp
            .query()
            .search::<VideoItem, _>("popular videos")
            .await
            .map_err(|e| {
                format!(
                    "Trending error: {}",
                    last_err.unwrap_or_else(|| e.to_string())
                )
            })?;

        Ok(fallback
            .items
            .items
            .into_iter()
            .filter(|it| !it.is_short)
            .map(|it| map_video_item(it, None, false))
            .collect())
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

        let avatar_url = pick_thumbnail_url(&ch.avatar, 128);
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
            avatar_url,
        };

        let fallback = ChannelFallback {
            id: &details.id,
            name: &details.name,
            avatar_url: &details.avatar_url,
        };

        let videos = ch
            .content
            .items
            .into_iter()
            .map(|it| map_video_item(it, Some(&fallback), false))
            .collect();
        Ok((details, videos))
    }

    pub async fn channel_shorts(
        &self,
        channel_id: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<Vec<VideoSummary>, String> {
        let ch = self
            .rp
            .query()
            .channel_videos_tab(channel_id, ChannelVideoTab::Shorts)
            .await
            .map_err(|e| format!("Channel shorts error: {e}"))?;

        let ch_avatar = pick_thumbnail_url(&ch.avatar, 128);
        let id_str = if !ch.id.is_empty() {
            ch.id.as_str()
        } else {
            fallback_details.map(|d| d.id.as_str()).unwrap_or(channel_id)
        };
        let name_str = if !ch.name.is_empty() {
            ch.name.as_str()
        } else {
            fallback_details.map(|d| d.name.as_str()).unwrap_or("")
        };
        let av_str = if !ch_avatar.is_empty() {
            ch_avatar.as_str()
        } else {
            fallback_details
                .map(|d| d.avatar_url.as_str())
                .unwrap_or("")
        };

        let fallback = ChannelFallback {
            id: id_str,
            name: name_str,
            avatar_url: av_str,
        };

        let videos = ch
            .content
            .items
            .into_iter()
            .map(|it| map_video_item(it, Some(&fallback), true))
            .collect();
        Ok(videos)
    }

    pub async fn channel_tab_ordered(
        &self,
        channel_id: &str,
        is_shorts: bool,
        order: ChannelSortOrder,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<Vec<VideoSummary>, String> {
        let tab = if is_shorts {
            ChannelVideoTab::Shorts
        } else {
            ChannelVideoTab::Videos
        };
        let rp_order = match order {
            ChannelSortOrder::Newest => ChannelOrder::Latest,
            ChannelSortOrder::Oldest => ChannelOrder::Oldest,
            ChannelSortOrder::Popular => ChannelOrder::Popular,
        };

        if order == ChannelSortOrder::Newest {
            if is_shorts {
                return self.channel_shorts(channel_id, fallback_details).await;
            } else {
                let (_, vids) = self.channel_videos(channel_id).await?;
                return Ok(vids);
            }
        }

        let paginator = self
            .rp
            .query()
            .channel_videos_tab_order(channel_id, tab, rp_order)
            .await
            .map_err(|e| format!("Channel sort error: {e}"))?;

        let fallback = fallback_details.map(|d| ChannelFallback {
            id: &d.id,
            name: &d.name,
            avatar_url: &d.avatar_url,
        });

        let videos = paginator
            .items
            .into_iter()
            .map(|it| map_video_item(it, fallback.as_ref(), is_shorts))
            .collect();
        Ok(videos)
    }

    pub async fn channel_search(
        &self,
        channel_id: &str,
        query: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<Vec<VideoSummary>, String> {
        let ch = self
            .rp
            .query()
            .channel_search(channel_id, query)
            .await
            .map_err(|e| format!("Channel search error: {e}"))?;

        let ch_avatar = pick_thumbnail_url(&ch.avatar, 128);
        let id_str = if !ch.id.is_empty() {
            ch.id.as_str()
        } else {
            fallback_details.map(|d| d.id.as_str()).unwrap_or(channel_id)
        };
        let name_str = if !ch.name.is_empty() {
            ch.name.as_str()
        } else {
            fallback_details.map(|d| d.name.as_str()).unwrap_or("")
        };
        let av_str = if !ch_avatar.is_empty() {
            ch_avatar.as_str()
        } else {
            fallback_details
                .map(|d| d.avatar_url.as_str())
                .unwrap_or("")
        };

        let fallback = ChannelFallback {
            id: id_str,
            name: name_str,
            avatar_url: av_str,
        };

        let videos = ch
            .content
            .items
            .into_iter()
            .map(|it| map_video_item(it, Some(&fallback), false))
            .collect();
        Ok(videos)
    }

    pub async fn subscriptions_feed(
        self: &Arc<Self>,
        subs: Vec<SubscribedChannel>,
    ) -> Result<Vec<VideoSummary>, String> {
        if subs.is_empty() {
            return Ok(Vec::new());
        }

        let sem = Arc::new(Semaphore::new(5));
        let mut set = JoinSet::new();

        for sub in subs {
            let engine = self.clone();
            let sem_cl = sem.clone();
            set.spawn(async move {
                let _permit = sem_cl.acquire_owned().await.ok()?;
                match engine.channel_videos(&sub.id).await {
                    Ok((details, mut vids)) => {
                        for v in &mut vids {
                            if v.channel_id.is_empty() {
                                v.channel_id = sub.id.clone();
                            }
                            if v.channel_name.is_empty() || v.channel_name == "Unknown" {
                                v.channel_name = if !details.name.is_empty() {
                                    details.name.clone()
                                } else {
                                    sub.name.clone()
                                };
                            }
                            if v.channel_avatar_url.is_empty() {
                                v.channel_avatar_url = if !details.avatar_url.is_empty() {
                                    details.avatar_url.clone()
                                } else {
                                    sub.avatar_url.clone()
                                };
                            }
                        }
                        vids.truncate(12);
                        Some(vids)
                    }
                    Err(_) => None,
                }
            });
        }

        let mut merged = Vec::new();
        let mut seen = HashSet::new();

        while let Some(res) = set.join_next().await {
            if let Ok(Some(vids)) = res {
                for v in vids {
                    if seen.insert(v.id.clone()) {
                        merged.push(v);
                    }
                }
            }
        }

        merged.sort_by(|a, b| {
            b.published_unix
                .unwrap_or(0)
                .cmp(&a.published_unix.unwrap_or(0))
        });

        Ok(merged)
    }
}

pub async fn download_with_ytdlp(
    video_id: &str,
    quality: Quality,
    downloads_dir: PathBuf,
) -> Result<PathBuf, String> {
    let _ = std::fs::create_dir_all(&downloads_dir);
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let out_template = downloads_dir
        .join("%(title).100s [%(id)s].%(ext)s")
        .to_string_lossy()
        .to_string();

    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--no-playlist")
        .arg("-o")
        .arg(&out_template)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    match quality {
        Quality::AudioOnly => {
            cmd.arg("-x").arg("--audio-format").arg("m4a");
        }
        other => {
            if let Some(fmt) = other.ytdl_format() {
                cmd.arg("-f").arg(fmt);
            }
        }
    }

    cmd.arg(&url);

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Could not run yt-dlp: {e}"))?;

    if output.status.success() {
        Ok(downloads_dir)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let short_err = stderr
            .lines()
            .last()
            .unwrap_or("yt-dlp exited with an error");
        Err(short_err.to_string())
    }
}

struct ChannelFallback<'a> {
    id: &'a str,
    name: &'a str,
    avatar_url: &'a str,
}

fn pick_thumbnail_url(thumbs: &[Thumbnail], min_width: u32) -> String {
    if thumbs.is_empty() {
        return String::new();
    }
    if let Some(found) = thumbs.iter().find(|t| t.width >= min_width) {
        return found.url.clone();
    }
    thumbs.last().map(|t| t.url.clone()).unwrap_or_default()
}

fn map_video_item(
    item: VideoItem,
    fallback: Option<&ChannelFallback<'_>>,
    force_short: bool,
) -> VideoSummary {
    let duration_secs = item.duration.map(|d| d as u64);
    let duration_text = duration_secs.map(format_duration).unwrap_or_default();

    let view_count = item.view_count;
    let view_count_text = view_count
        .map(format_count)
        .map(|s| format!("{s} views"))
        .unwrap_or_default();

    let thumbnail_url = pick_thumbnail_url(&item.thumbnail, 480);

    let (mut channel_id, mut channel_name, mut channel_avatar_url) =
        if let Some(c) = item.channel {
            let avatar = pick_thumbnail_url(&c.avatar, 88);
            (c.id, c.name, avatar)
        } else {
            (String::new(), String::new(), String::new())
        };

    if let Some(fb) = fallback {
        if channel_id.is_empty() {
            channel_id = fb.id.to_string();
        }
        if channel_name.is_empty() {
            channel_name = fb.name.to_string();
        }
        if channel_avatar_url.is_empty() {
            channel_avatar_url = fb.avatar_url.to_string();
        }
    }

    let description_snippet = item.short_description.unwrap_or_default();
    let published_text = item.publish_date_txt.unwrap_or_default();
    let published_unix = item.publish_date.map(|dt| dt.unix_timestamp());
    let is_short = force_short || item.is_short;

    VideoSummary {
        id: item.id,
        title: item.name,
        channel_id,
        channel_name,
        channel_avatar_url,
        duration_seconds: duration_secs,
        duration_text,
        view_count_text,
        view_count,
        published_text,
        published_unix,
        is_short,
        thumbnail_url,
        description_snippet,
    }
}
