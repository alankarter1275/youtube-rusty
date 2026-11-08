use super::models::{
    format_count, format_duration, ChannelDetails, ChannelSearchSummary, ChannelSortOrder,
    DurationFilter, PlaylistSearchSummary, SearchItem, SearchSortOrder, SearchTab,
    UploadDateFilter, VideoSummary,
};
use super::subscriptions::SubscribedChannel;
use crate::player::mpv::{Quality, ResolvedPlayback};
use rustypipe::client::{ClientType, RustyPipe};
use rustypipe::model::paginator::{ContinuationEndpoint, Paginator};
use rustypipe::model::{
    AudioCodec, ChannelItem, PlaylistItem, Thumbnail, UrlTarget, VideoCodec, VideoItem,
    VideoPlayer, YouTubeItem,
};
use rustypipe::param::search_filter::{ItemType, Length, Order, SearchFilter, UploadDate};
use rustypipe::param::{ChannelOrder, ChannelVideoTab};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const RP_IOS_UA: &str =
    "com.google.ios.youtube/20.03.02 (iPhone16,2; U; CPU iOS 18_2_1 like Mac OS X)";
const RP_ANDROID_UA: &str = "com.google.android.youtube/19.44.38 (Linux; U; Android 11) gzip";
const RP_TV_UA: &str = "Mozilla/5.0 (SMART-TV; Linux; Tizen 5.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/5.0 NativeTVAds Safari/538.1";

const VR_CLIENT_VERSION: &str = "1.62.27";
const VR_UA: &str = "com.google.android.apps.youtube.vr.oculus/1.62.27 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";

const MODERN_IOS_VERSION: &str = "20.22.1";
const MODERN_IOS_UA: &str =
    "com.google.ios.youtube/20.22.1 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X)";

#[derive(Clone, Debug)]
pub struct PageCursor {
    pub ctoken: String,
    pub endpoint: ContinuationEndpoint,
    pub visitor_data: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PagedVideos {
    pub items: Vec<VideoSummary>,
    pub next: Option<PageCursor>,
}

#[derive(Clone, Debug)]
pub struct PagedSearchItems {
    pub items: Vec<SearchItem>,
    pub next: Option<PageCursor>,
}

#[derive(Clone, Debug)]
pub struct PagedTrending {
    pub items: Vec<VideoSummary>,
    pub cursors: Vec<PageCursor>,
}

#[derive(Clone, Debug)]
pub struct RemotePlaylist {
    pub id: String,
    pub name: String,
    pub channel_name: String,
    pub video_count_text: String,
    pub videos: Vec<VideoSummary>,
    pub next: Option<PageCursor>,
}

#[derive(Clone, Debug)]
pub enum ResolvedUrl {
    Video(VideoSummary),
    Channel { id: String, name: String },
    Playlist { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimpleVideoCodec {
    H264,
    Vp9,
    Mp4v,
    Av1,
    Unknown,
}

impl SimpleVideoCodec {
    /// Hardware-friendly priority for modest CPUs/GPUs: H.264 > VP9 > MP4V > Unknown > AV1.
    fn hardware_rank(self) -> u8 {
        match self {
            SimpleVideoCodec::H264 => 5,
            SimpleVideoCodec::Vp9 => 4,
            SimpleVideoCodec::Mp4v => 3,
            SimpleVideoCodec::Unknown => 2,
            SimpleVideoCodec::Av1 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimpleAudioCodec {
    Aac,
    Opus,
    Other,
}

#[derive(Clone, Debug)]
struct RawVideoCandidate {
    url: String,
    height: u32,
    fps: u32,
    bitrate: u32,
    hdr: bool,
    is_mp4: bool,
    codec: SimpleVideoCodec,
}

#[derive(Clone, Debug)]
struct RawAudioCandidate {
    url: String,
    bitrate: u32,
    is_m4a: bool,
    codec: SimpleAudioCodec,
}

#[derive(Clone, Debug)]
struct CandidatePool {
    muxed: Vec<RawVideoCandidate>,
    adaptive_video: Vec<RawVideoCandidate>,
    audio: Vec<RawAudioCandidate>,
    hls_url: Option<String>,
    user_agent: String,
    duration_secs: Option<u64>,
    title: Option<String>,
    channel_id: String,
    channel_name: String,
    thumbnail_url: String,
    view_count: Option<u64>,
}

impl CandidatePool {
    fn has_playable_streams(&self) -> bool {
        !self.muxed.is_empty()
            || (!self.adaptive_video.is_empty() && !self.audio.is_empty())
            || !self.audio.is_empty()
            || self.hls_url.is_some()
    }
}

fn cursor_from_paginator<T>(p: &Paginator<T>) -> Option<PageCursor> {
    p.ctoken.as_ref().map(|ctoken| PageCursor {
        ctoken: ctoken.clone(),
        endpoint: p.endpoint,
        visitor_data: p.visitor_data.clone(),
    })
}

pub struct Engine {
    rp: RustyPipe,
    http: reqwest::Client,
}

impl Engine {
    pub fn new(storage_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(storage_dir);
        let rp = RustyPipe::builder()
            .storage_dir(storage_dir)
            .build()
            .unwrap_or_else(|_| RustyPipe::new());
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { rp, http }
    }

    /// Resolves direct YouTube media stream URLs in pure Rust without `yt-dlp` or Python.
    pub async fn resolve_playback(
        &self,
        video_id: &str,
        quality: Quality,
    ) -> Result<ResolvedPlayback, String> {
        let pool = self.fetch_candidate_pool(video_id).await?;
        select_best_playback(&pool, quality)
            .ok_or_else(|| "No playable streams available for this video.".to_string())
    }

    async fn fetch_candidate_pool(&self, video_id: &str) -> Result<CandidatePool, String> {
        // 1. Direct Innertube ANDROID_VR (fast single POST, un-ciphered 360p/720p/1080p H.264/VP9 + AAC/Opus)
        let mut last_err = match self.fetch_innertube_vr(video_id).await {
            Ok(pool) if pool.has_playable_streams() => return Ok(pool),
            Ok(_) => "No streams returned by VR player".to_string(),
            Err(e) => e,
        };

        // 2. Modern iOS Innertube (fast single POST, un-ciphered streams + HLS live manifests)
        match self.fetch_innertube_modern_ios(video_id).await {
            Ok(pool) if pool.has_playable_streams() => return Ok(pool),
            Ok(_) => {}
            Err(e) => last_err = e,
        }

        // 3. RustyPipe built-in clients (iOS -> Android -> TV)
        for (ctype, ua) in [
            (ClientType::Ios, RP_IOS_UA),
            (ClientType::Android, RP_ANDROID_UA),
            (ClientType::Tv, RP_TV_UA),
        ] {
            match self.rp.query().player_from_clients(video_id, &[ctype]).await {
                Ok(player) => {
                    let pool = pool_from_rustypipe(player, ua);
                    if pool.has_playable_streams() {
                        return Ok(pool);
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
        }

        if last_err.is_empty() {
            Err("Could not extract playable stream URLs.".to_string())
        } else {
            Err(format!("Stream extraction failed: {last_err}"))
        }
    }

    async fn fetch_innertube_vr(&self, video_id: &str) -> Result<CandidatePool, String> {
        let body = json!({
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": {
                    "clientName": "ANDROID_VR",
                    "clientVersion": VR_CLIENT_VERSION,
                    "deviceMake": "Oculus",
                    "deviceModel": "Quest 3",
                    "osName": "Android",
                    "osVersion": "12L",
                    "androidSdkVersion": 32,
                    "hl": "en",
                    "gl": "US"
                }
            }
        });
        self.post_innertube_player(&body, VR_UA, "28").await
    }

    async fn fetch_innertube_modern_ios(&self, video_id: &str) -> Result<CandidatePool, String> {
        let body = json!({
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": {
                    "clientName": "IOS",
                    "clientVersion": MODERN_IOS_VERSION,
                    "deviceMake": "Apple",
                    "deviceModel": "iPhone16,2",
                    "osName": "iPhone",
                    "osVersion": "18.3.2.22D82",
                    "hl": "en",
                    "gl": "US"
                }
            }
        });
        self.post_innertube_player(&body, MODERN_IOS_UA, "5").await
    }

    async fn post_innertube_player(
        &self,
        body: &Value,
        ua: &str,
        client_name_id: &str,
    ) -> Result<CandidatePool, String> {
        let client_ver = body
            .pointer("/context/client/clientVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("1.0");

        let resp = self
            .http
            .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
            .header("User-Agent", ua)
            .header("Content-Type", "application/json")
            .header("X-YouTube-Client-Name", client_name_id)
            .header("X-YouTube-Client-Version", client_ver)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Network error: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Player HTTP error: {e}"))?;

        let val: Value = resp
            .json()
            .await
            .map_err(|e| format!("Invalid player JSON: {e}"))?;

        let status = val
            .pointer("/playabilityStatus/status")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if status != "OK" && status != "LIVE_STREAM_OFFLINE" {
            let reason = val
                .pointer("/playabilityStatus/reason")
                .and_then(|r| r.as_str())
                .unwrap_or(status);
            return Err(reason.to_string());
        }

        let mut muxed = Vec::new();
        let mut adaptive_video = Vec::new();
        let mut audio = Vec::new();

        if let Some(formats) = val
            .pointer("/streamingData/formats")
            .and_then(|f| f.as_array())
        {
            for fmt in formats {
                if let Some(vc) = parse_json_video_stream(fmt) {
                    muxed.push(vc);
                }
            }
        }

        if let Some(adap) = val
            .pointer("/streamingData/adaptiveFormats")
            .and_then(|f| f.as_array())
        {
            for fmt in adap {
                let mime = fmt.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
                if mime.starts_with("video/") {
                    if let Some(vc) = parse_json_video_stream(fmt) {
                        adaptive_video.push(vc);
                    }
                } else if mime.starts_with("audio/") {
                    if let Some(ac) = parse_json_audio_stream(fmt) {
                        audio.push(ac);
                    }
                }
            }
        }

        let hls_url = val
            .pointer("/streamingData/hlsManifestUrl")
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let duration_secs = val
            .pointer("/videoDetails/lengthSeconds")
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&d| d > 0);

        let title = val
            .pointer("/videoDetails/title")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let channel_id = val
            .pointer("/videoDetails/channelId")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let channel_name = val
            .pointer("/videoDetails/author")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let view_count = val
            .pointer("/videoDetails/viewCount")
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<u64>().ok());

        let thumbnail_url = val
            .pointer("/videoDetails/thumbnail/thumbnails")
            .and_then(|arr| arr.as_array())
            .and_then(|arr| arr.last())
            .and_then(|t| t.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string();

        Ok(CandidatePool {
            muxed,
            adaptive_video,
            audio,
            hls_url,
            user_agent: ua.to_string(),
            duration_secs,
            title,
            channel_id,
            channel_name,
            thumbnail_url,
            view_count,
        })
    }

    /// Pure-Rust downloader using `RustyPipe`/Innertube stream resolution + `reqwest` chunk streaming.
    /// Zero `yt-dlp` or Python required. Uses `ffmpeg -c copy` only if available for HD adaptive streams.
    pub async fn download_video(
        &self,
        video_id: &str,
        title: &str,
        quality: Quality,
        downloads_dir: PathBuf,
    ) -> Result<PathBuf, String> {
        tokio::fs::create_dir_all(&downloads_dir)
            .await
            .map_err(|e| format!("Could not create Downloads folder: {e}"))?;

        let pool = self.fetch_candidate_pool(video_id).await?;
        let base_name = sanitize_filename(if title.trim().is_empty() {
            pool.title.as_deref().unwrap_or(video_id)
        } else {
            title
        });

        if quality == Quality::AudioOnly {
            let best_aud = pick_best_audio(&pool.audio)
                .ok_or_else(|| "No downloadable audio stream found.".to_string())?;
            let ext = if best_aud.is_m4a { "m4a" } else { "webm" };
            let out_path = downloads_dir.join(format!("{base_name} [{video_id}].{ext}"));
            self.stream_url_to_file(&best_aud.url, &pool.user_agent, &out_path)
                .await?;
            return Ok(out_path);
        }

        let target_h = quality.target_height().unwrap_or(360);
        let best_muxed = pick_best_video(&pool.muxed, target_h);
        let best_adap_v = pick_best_video(&pool.adaptive_video, target_h);
        let best_aud = pick_best_audio(&pool.audio);

        // For 720p/1080p, use lossless ffmpeg remux if ffmpeg is installed and adaptive beats muxed
        if target_h > 480 {
            if let (Some(v), Some(a)) = (best_adap_v, best_aud) {
                let muxed_h = best_muxed.map(|m| m.height).unwrap_or(0);
                if v.height > muxed_h && has_ffmpeg().await {
                    let out_ext = if v.is_mp4 && a.is_m4a { "mp4" } else { "mkv" };
                    let out_path =
                        downloads_dir.join(format!("{base_name} [{video_id}].{out_ext}"));
                    let tmp_v = downloads_dir.join(format!(".yr_tmp_{video_id}_v.part"));
                    let tmp_a = downloads_dir.join(format!(".yr_tmp_{video_id}_a.part"));

                    let dl_res = async {
                        self.stream_url_to_file(&v.url, &pool.user_agent, &tmp_v)
                            .await?;
                        self.stream_url_to_file(&a.url, &pool.user_agent, &tmp_a)
                            .await?;
                        let status = tokio::process::Command::new("ffmpeg")
                            .arg("-y")
                            .arg("-loglevel")
                            .arg("error")
                            .arg("-i")
                            .arg(&tmp_v)
                            .arg("-i")
                            .arg(&tmp_a)
                            .arg("-c")
                            .arg("copy")
                            .arg(&out_path)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::piped())
                            .status()
                            .await
                            .map_err(|e| format!("ffmpeg error: {e}"))?;
                        if !status.success() {
                            return Err("ffmpeg could not combine video and audio".to_string());
                        }
                        Ok(())
                    }
                    .await;

                    let _ = tokio::fs::remove_file(&tmp_v).await;
                    let _ = tokio::fs::remove_file(&tmp_a).await;

                    if dl_res.is_ok() {
                        return Ok(out_path);
                    }
                }
            }
        }

        // Pure-Rust direct progressive MP4 download (zero external tools required)
        if let Some(muxed) = best_muxed.or_else(|| pool.muxed.first()) {
            let ext = if muxed.is_mp4 { "mp4" } else { "webm" };
            let out_path = downloads_dir.join(format!("{base_name} [{video_id}].{ext}"));
            self.stream_url_to_file(&muxed.url, &pool.user_agent, &out_path)
                .await?;
            return Ok(out_path);
        }

        Err("No downloadable file stream available (install ffmpeg for HD adaptive downloads).".to_string())
    }

    async fn stream_url_to_file(
        &self,
        url: &str,
        user_agent: &str,
        dest: &Path,
    ) -> Result<(), String> {
        let mut resp = self
            .http
            .get(url)
            .header("User-Agent", user_agent)
            .header("Referer", "https://www.youtube.com/")
            .send()
            .await
            .map_err(|e| format!("Connection error: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Download HTTP error: {e}"))?;

        let tmp_dest = dest.with_extension("part");
        let mut file = tokio::fs::File::create(&tmp_dest)
            .await
            .map_err(|e| format!("File write error: {e}"))?;

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("Stream read error: {e}"))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Disk write error: {e}"))?;
        }
        file.flush()
            .await
            .map_err(|e| format!("Disk flush error: {e}"))?;
        drop(file);

        tokio::fs::rename(&tmp_dest, dest)
            .await
            .map_err(|e| format!("Could not finalize downloaded file: {e}"))?;
        Ok(())
    }

    pub async fn search_suggestions(&self, query: &str) -> Result<Vec<String>, String> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        self.rp
            .query()
            .search_suggestion(q)
            .await
            .map_err(|e| format!("Suggestion error: {e}"))
    }

    pub async fn search_items(
        &self,
        query: &str,
        tab: SearchTab,
        date: UploadDateFilter,
        duration: DurationFilter,
        sort: SearchSortOrder,
    ) -> Result<PagedSearchItems, String> {
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

        let is_default_filters =
            rp_date.is_none() && rp_len.is_none() && rp_sort.is_none();

        match tab {
            SearchTab::All => {
                let paginator = if is_default_filters {
                    self.rp
                        .query()
                        .search::<YouTubeItem, _>(query)
                        .await
                        .map_err(|e| format!("Search error: {e}"))?
                        .items
                } else {
                    let filter = SearchFilter::new()
                        .date_opt(rp_date)
                        .length_opt(rp_len)
                        .sort_opt(rp_sort);
                    self.rp
                        .query()
                        .search_filter::<YouTubeItem, _>(query, &filter)
                        .await
                        .map_err(|e| format!("Search error: {e}"))?
                        .items
                };
                let next = cursor_from_paginator(&paginator);
                let items = paginator
                    .items
                    .into_iter()
                    .map(map_youtube_item)
                    .collect();
                Ok(PagedSearchItems { items, next })
            }
            SearchTab::Videos => {
                let paged = self.search_filtered(query, date, duration, sort).await?;
                Ok(PagedSearchItems {
                    items: paged.items.into_iter().map(SearchItem::Video).collect(),
                    next: paged.next,
                })
            }
            SearchTab::Channels => {
                let filter = SearchFilter::new()
                    .item_type(ItemType::Channel)
                    .sort_opt(rp_sort);
                let paginator = self
                    .rp
                    .query()
                    .search_filter::<ChannelItem, _>(query, &filter)
                    .await
                    .map_err(|e| format!("Channel search error: {e}"))?
                    .items;
                let next = cursor_from_paginator(&paginator);
                let items = paginator
                    .items
                    .into_iter()
                    .map(|c| SearchItem::Channel(map_channel_item(c)))
                    .collect();
                Ok(PagedSearchItems { items, next })
            }
            SearchTab::Playlists => {
                let filter = SearchFilter::new()
                    .item_type(ItemType::Playlist)
                    .sort_opt(rp_sort);
                let paginator = self
                    .rp
                    .query()
                    .search_filter::<PlaylistItem, _>(query, &filter)
                    .await
                    .map_err(|e| format!("Playlist search error: {e}"))?
                    .items;
                let next = cursor_from_paginator(&paginator);
                let items = paginator
                    .items
                    .into_iter()
                    .map(|p| SearchItem::Playlist(map_playlist_item(p)))
                    .collect();
                Ok(PagedSearchItems { items, next })
            }
        }
    }

    pub async fn load_more_search_items(
        &self,
        cursor: &PageCursor,
        tab: SearchTab,
    ) -> Result<PagedSearchItems, String> {
        match tab {
            SearchTab::All => {
                let paginator = self
                    .rp
                    .query()
                    .continuation::<YouTubeItem, _>(
                        &cursor.ctoken,
                        cursor.endpoint,
                        cursor.visitor_data.as_deref(),
                    )
                    .await
                    .map_err(|e| format!("Load more error: {e}"))?;
                let next = cursor_from_paginator(&paginator);
                let items = paginator
                    .items
                    .into_iter()
                    .map(map_youtube_item)
                    .collect();
                Ok(PagedSearchItems { items, next })
            }
            SearchTab::Videos => {
                let paged = self.load_more(cursor, None, false, false).await?;
                Ok(PagedSearchItems {
                    items: paged.items.into_iter().map(SearchItem::Video).collect(),
                    next: paged.next,
                })
            }
            SearchTab::Channels => {
                let paginator = self
                    .rp
                    .query()
                    .continuation::<ChannelItem, _>(
                        &cursor.ctoken,
                        cursor.endpoint,
                        cursor.visitor_data.as_deref(),
                    )
                    .await
                    .map_err(|e| format!("Load more error: {e}"))?;
                let next = cursor_from_paginator(&paginator);
                let items = paginator
                    .items
                    .into_iter()
                    .map(|c| SearchItem::Channel(map_channel_item(c)))
                    .collect();
                Ok(PagedSearchItems { items, next })
            }
            SearchTab::Playlists => {
                let paginator = self
                    .rp
                    .query()
                    .continuation::<PlaylistItem, _>(
                        &cursor.ctoken,
                        cursor.endpoint,
                        cursor.visitor_data.as_deref(),
                    )
                    .await
                    .map_err(|e| format!("Load more error: {e}"))?;
                let next = cursor_from_paginator(&paginator);
                let items = paginator
                    .items
                    .into_iter()
                    .map(|p| SearchItem::Playlist(map_playlist_item(p)))
                    .collect();
                Ok(PagedSearchItems { items, next })
            }
        }
    }

    pub async fn fetch_playlist(&self, playlist_id: &str) -> Result<RemotePlaylist, String> {
        let pl = self
            .rp
            .query()
            .playlist(playlist_id)
            .await
            .map_err(|e| format!("Could not load playlist: {e}"))?;

        let channel_name = pl.channel.map(|c| c.name).unwrap_or_default();

        let video_count_text = if pl.video_count > 0 {
            format!("{} videos", format_count(pl.video_count))
        } else {
            format!("{} videos", pl.videos.items.len())
        };

        let next = cursor_from_paginator(&pl.videos);
        let videos = pl
            .videos
            .items
            .into_iter()
            .map(|it| map_video_item(it, None, false))
            .collect();

        Ok(RemotePlaylist {
            id: pl.id,
            name: pl.name,
            channel_name,
            video_count_text,
            videos,
            next,
        })
    }

    pub async fn resolve_youtube_url(&self, raw_url: &str) -> Result<ResolvedUrl, String> {
        let trimmed = raw_url.trim();
        let normalized = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        };

        if let Some(quick) = quick_parse_youtube_ids(&normalized) {
            match quick {
                QuickTarget::Video(vid) => {
                    let summary = self.fetch_video_summary(&vid).await?;
                    return Ok(ResolvedUrl::Video(summary));
                }
                QuickTarget::Playlist(pid) => {
                    return Ok(ResolvedUrl::Playlist { id: pid });
                }
            }
        }

        let target = self
            .rp
            .query()
            .resolve_url(&normalized, false)
            .await
            .map_err(|e| format!("Could not open link: {e}"))?;

        match target {
            UrlTarget::Video { id, .. } => {
                let summary = self.fetch_video_summary(&id).await?;
                Ok(ResolvedUrl::Video(summary))
            }
            UrlTarget::Channel { id } => Ok(ResolvedUrl::Channel {
                id,
                name: "Channel".to_string(),
            }),
            UrlTarget::Playlist { id } | UrlTarget::Album { id } => {
                Ok(ResolvedUrl::Playlist { id })
            }
        }
    }

    pub async fn fetch_video_summary(&self, video_id: &str) -> Result<VideoSummary, String> {
        if let Ok(pool) = self.fetch_candidate_pool(video_id).await {
            if let Some(title) = pool.title.filter(|t| !t.is_empty()) {
                let dur_secs = pool.duration_secs;
                let dur_txt = dur_secs.map(format_duration).unwrap_or_default();
                let view_txt = pool
                    .view_count
                    .map(format_count)
                    .map(|s| format!("{s} views"))
                    .unwrap_or_default();
                let thumb = if pool.thumbnail_url.is_empty() {
                    format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")
                } else {
                    pool.thumbnail_url
                };
                return Ok(VideoSummary {
                    id: video_id.to_string(),
                    title,
                    channel_id: pool.channel_id,
                    channel_name: pool.channel_name,
                    channel_avatar_url: String::new(),
                    duration_seconds: dur_secs,
                    duration_text: dur_txt,
                    view_count_text: view_txt,
                    view_count: pool.view_count,
                    published_text: String::new(),
                    published_unix: None,
                    is_short: false,
                    thumbnail_url: thumb,
                    description_snippet: String::new(),
                });
            }
        }

        let details = self
            .rp
            .query()
            .video_details(video_id)
            .await
            .map_err(|e| format!("Could not fetch video info: {e}"))?;

        let view_txt = format!("{} views", format_count(details.view_count));
        let ch_avatar = pick_thumbnail_url(&details.channel.avatar, 88);

        Ok(VideoSummary {
            id: details.id.clone(),
            title: details.name,
            channel_id: details.channel.id,
            channel_name: details.channel.name,
            channel_avatar_url: ch_avatar,
            duration_seconds: None,
            duration_text: String::new(),
            view_count_text: view_txt,
            view_count: Some(details.view_count),
            published_text: details.publish_date_txt.unwrap_or_default(),
            published_unix: details.publish_date.map(|d| d.unix_timestamp()),
            is_short: false,
            thumbnail_url: format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"),
            description_snippet: String::new(),
        })
    }

    pub async fn search_filtered(
        &self,
        query: &str,
        date: UploadDateFilter,
        duration: DurationFilter,
        sort: SearchSortOrder,
    ) -> Result<PagedVideos, String> {
        let is_default = date == UploadDateFilter::AnyTime
            && duration == DurationFilter::AnyLength
            && sort == SearchSortOrder::Relevance;

        let paginator = if is_default {
            self.rp
                .query()
                .search::<VideoItem, _>(query)
                .await
                .map_err(|e| format!("Search error: {e}"))?
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
        };

        let next = cursor_from_paginator(&paginator);
        let items = paginator
            .items
            .into_iter()
            .map(|it| map_video_item(it, None, false))
            .collect();

        Ok(PagedVideos { items, next })
    }

    pub async fn category_feed(&self, category: &str) -> Result<PagedVideos, String> {
        let filter = SearchFilter::new()
            .item_type(ItemType::Video)
            .date(UploadDate::Week);

        let paginator = match self
            .rp
            .query()
            .search_filter::<VideoItem, _>(category, &filter)
            .await
        {
            Ok(res) if !res.items.items.is_empty() => res.items,
            _ => {
                self.rp
                    .query()
                    .search::<VideoItem, _>(category)
                    .await
                    .map_err(|e| format!("Category error: {e}"))?
                    .items
            }
        };

        let next = cursor_from_paginator(&paginator);
        let items = paginator
            .items
            .into_iter()
            .filter(|it| !it.is_short)
            .map(|it| map_video_item(it, None, false))
            .collect();

        Ok(PagedVideos { items, next })
    }

    pub async fn load_more(
        &self,
        cursor: &PageCursor,
        fallback_details: Option<&ChannelDetails>,
        force_short: bool,
        filter_out_shorts: bool,
    ) -> Result<PagedVideos, String> {
        let paginator = self
            .rp
            .query()
            .continuation::<VideoItem, _>(
                &cursor.ctoken,
                cursor.endpoint,
                cursor.visitor_data.as_deref(),
            )
            .await
            .map_err(|e| format!("Load more error: {e}"))?;

        let next = cursor_from_paginator(&paginator);
        let fallback = fallback_details.map(|d| ChannelFallback {
            id: &d.id,
            name: &d.name,
            avatar_url: &d.avatar_url,
        });

        let items = paginator
            .items
            .into_iter()
            .filter(|it| !filter_out_shorts || !it.is_short)
            .map(|it| map_video_item(it, fallback.as_ref(), force_short))
            .collect();

        Ok(PagedVideos { items, next })
    }

    /// Lightweight initial Trending page (~16-20 videos from 2 concurrent discovery queries)
    /// plus continuation cursors so additional videos are fetched only when scrolling near bottom.
    pub async fn trending_initial(&self) -> Result<PagedTrending, String> {
        let topics = ["music", "technology"];
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
                    .map(|r| r.items)
                    .map_err(|e| e.to_string());
                (idx, res)
            });
        }

        let mut buckets: Vec<Vec<VideoSummary>> = vec![Vec::new(); topics.len()];
        let mut cursors = Vec::new();
        let mut last_err = None;

        while let Some(joined) = join_set.join_next().await {
            if let Ok((idx, res)) = joined {
                match res {
                    Ok(paginator) => {
                        if let Some(c) = cursor_from_paginator(&paginator) {
                            cursors.push(c);
                        }
                        buckets[idx] = paginator
                            .items
                            .into_iter()
                            .filter(|it| !it.is_short && it.duration.unwrap_or(120) >= 60)
                            .map(|it| map_video_item(it, None, false))
                            .collect();
                    }
                    Err(err) => last_err = Some(err),
                }
            }
        }

        let mut merged = Vec::new();
        let mut seen = HashSet::new();
        let max_len = buckets.iter().map(|b| b.len()).max().unwrap_or(0);
        for i in 0..max_len {
            for bucket in &buckets {
                if let Some(v) = bucket.get(i) {
                    if seen.insert(v.id.clone()) {
                        merged.push(v.clone());
                        if merged.len() >= 16 {
                            return Ok(PagedTrending {
                                items: merged,
                                cursors,
                            });
                        }
                    }
                }
            }
        }

        if !merged.is_empty() {
            return Ok(PagedTrending {
                items: merged,
                cursors,
            });
        }

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

        let mut fallback_cursors = Vec::new();
        if let Some(c) = cursor_from_paginator(&fallback.items) {
            fallback_cursors.push(c);
        }

        let items = fallback
            .items
            .items
            .into_iter()
            .filter(|it| !it.is_short)
            .map(|it| map_video_item(it, None, false))
            .collect();

        Ok(PagedTrending {
            items,
            cursors: fallback_cursors,
        })
    }

    pub async fn trending_more(
        &self,
        cursors: Vec<PageCursor>,
        existing_ids: HashSet<String>,
    ) -> Result<PagedTrending, String> {
        if cursors.is_empty() {
            return Ok(PagedTrending {
                items: Vec::new(),
                cursors: Vec::new(),
            });
        }

        let mut next_cursors = Vec::new();
        let mut seen = existing_ids;
        let mut added = Vec::new();

        // Advance the first cursor and rotate remaining cursors to the front for next scroll
        let mut iter = cursors.into_iter();
        if let Some(first) = iter.next() {
            next_cursors.extend(iter);
            if let Ok(paged) = self.load_more(&first, None, false, true).await {
                if let Some(nc) = paged.next {
                    next_cursors.push(nc);
                }
                for v in paged.items {
                    if v.duration_seconds.unwrap_or(120) >= 60 && seen.insert(v.id.clone()) {
                        added.push(v);
                    }
                }
            }
        }

        Ok(PagedTrending {
            items: added,
            cursors: next_cursors,
        })
    }

    pub async fn channel_videos(
        &self,
        channel_id: &str,
    ) -> Result<(ChannelDetails, PagedVideos), String> {
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

        let next = cursor_from_paginator(&ch.content);
        let items = ch
            .content
            .items
            .into_iter()
            .map(|it| map_video_item(it, Some(&fallback), false))
            .collect();
        Ok((details, PagedVideos { items, next }))
    }

    pub async fn channel_shorts(
        &self,
        channel_id: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<PagedVideos, String> {
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

        let next = cursor_from_paginator(&ch.content);
        let items = ch
            .content
            .items
            .into_iter()
            .map(|it| map_video_item(it, Some(&fallback), true))
            .collect();
        Ok(PagedVideos { items, next })
    }

    pub async fn channel_tab_ordered(
        &self,
        channel_id: &str,
        is_shorts: bool,
        order: ChannelSortOrder,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<PagedVideos, String> {
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
                let (_, paged) = self.channel_videos(channel_id).await?;
                return Ok(paged);
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

        let next = cursor_from_paginator(&paginator);
        let items = paginator
            .items
            .into_iter()
            .map(|it| map_video_item(it, fallback.as_ref(), is_shorts))
            .collect();
        Ok(PagedVideos { items, next })
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
                    Ok((details, paged)) => {
                        let mut vids = paged.items;
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

enum QuickTarget {
    Video(String),
    Playlist(String),
}

fn quick_parse_youtube_ids(url: &str) -> Option<QuickTarget> {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;

    if let Some(rest) = after_scheme.strip_prefix("youtu.be/") {
        let vid = rest.split(['?', '&', '/']).next().unwrap_or("");
        if vid.len() == 11 {
            return Some(QuickTarget::Video(vid.to_string()));
        }
    }

    let (_, path_and_query) = after_scheme.split_once('/')?;
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_and_query, ""),
    };

    if let Some(short_id) = path.strip_prefix("shorts/") {
        let vid = short_id.split(['/', '&']).next().unwrap_or("");
        if vid.len() == 11 {
            return Some(QuickTarget::Video(vid.to_string()));
        }
    }

    let mut v_param = None;
    let mut list_param = None;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "v" && v.len() == 11 {
                v_param = Some(v.to_string());
            } else if k == "list" && !v.is_empty() {
                list_param = Some(v.to_string());
            }
        }
    }

    if path.starts_with("playlist") {
        if let Some(pid) = list_param {
            return Some(QuickTarget::Playlist(pid));
        }
    }
    if let Some(vid) = v_param {
        return Some(QuickTarget::Video(vid));
    }
    if let Some(pid) = list_param {
        return Some(QuickTarget::Playlist(pid));
    }
    None
}

async fn has_ffmpeg() -> bool {
    tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'..='\x1f' => '_',
            _ => c,
        })
        .take(90)
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "video".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_json_video_stream(fmt: &Value) -> Option<RawVideoCandidate> {
    let url = fmt.get("url").and_then(|u| u.as_str())?.to_string();
    if url.is_empty() {
        return None;
    }
    let mime = fmt.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    let height = fmt.get("height").and_then(|h| h.as_u64()).unwrap_or(0) as u32;
    let fps = fmt.get("fps").and_then(|f| f.as_u64()).unwrap_or(30) as u32;
    let bitrate = fmt
        .get("averageBitrate")
        .or_else(|| fmt.get("bitrate"))
        .and_then(|b| b.as_u64())
        .unwrap_or(0) as u32;
    let qual_label = fmt
        .get("qualityLabel")
        .and_then(|q| q.as_str())
        .unwrap_or("");
    let hdr = qual_label.contains("HDR") || mime.contains("vp9.2");
    let is_mp4 = mime.starts_with("video/mp4");
    let codec = if mime.contains("avc1") {
        SimpleVideoCodec::H264
    } else if mime.contains("vp9") || mime.contains("vp09") {
        SimpleVideoCodec::Vp9
    } else if mime.contains("av01") {
        SimpleVideoCodec::Av1
    } else if mime.contains("mp4v") {
        SimpleVideoCodec::Mp4v
    } else {
        SimpleVideoCodec::Unknown
    };

    Some(RawVideoCandidate {
        url,
        height,
        fps,
        bitrate,
        hdr,
        is_mp4,
        codec,
    })
}

fn parse_json_audio_stream(fmt: &Value) -> Option<RawAudioCandidate> {
    let url = fmt.get("url").and_then(|u| u.as_str())?.to_string();
    if url.is_empty() {
        return None;
    }
    // Prefer original/default audio track when multi-language tracks are present
    if let Some(track_type) = fmt
        .pointer("/audioTrack/audioIsDefault")
        .and_then(|b| b.as_bool())
    {
        if !track_type {
            return None;
        }
    }
    let mime = fmt.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    let bitrate = fmt
        .get("averageBitrate")
        .or_else(|| fmt.get("bitrate"))
        .and_then(|b| b.as_u64())
        .unwrap_or(0) as u32;
    let is_m4a = mime.starts_with("audio/mp4");
    let codec = if mime.contains("mp4a") {
        SimpleAudioCodec::Aac
    } else if mime.contains("opus") {
        SimpleAudioCodec::Opus
    } else {
        SimpleAudioCodec::Other
    };

    Some(RawAudioCandidate {
        url,
        bitrate,
        is_m4a,
        codec,
    })
}

fn pool_from_rustypipe(player: VideoPlayer, ua: &str) -> CandidatePool {
    let muxed = player
        .video_streams
        .into_iter()
        .map(|s| RawVideoCandidate {
            url: s.url,
            height: s.height,
            fps: s.fps as u32,
            bitrate: if s.average_bitrate > 0 {
                s.average_bitrate
            } else {
                s.bitrate
            },
            hdr: s.hdr,
            is_mp4: s.mime.starts_with("video/mp4"),
            codec: map_rp_video_codec(s.codec),
        })
        .collect();

    let adaptive_video = player
        .video_only_streams
        .into_iter()
        .map(|s| RawVideoCandidate {
            url: s.url,
            height: s.height,
            fps: s.fps as u32,
            bitrate: if s.average_bitrate > 0 {
                s.average_bitrate
            } else {
                s.bitrate
            },
            hdr: s.hdr,
            is_mp4: s.mime.starts_with("video/mp4"),
            codec: map_rp_video_codec(s.codec),
        })
        .collect();

    let audio = player
        .audio_streams
        .into_iter()
        .map(|s| RawAudioCandidate {
            url: s.url,
            bitrate: if s.average_bitrate > 0 {
                s.average_bitrate
            } else {
                s.bitrate
            },
            is_m4a: s.mime.starts_with("audio/mp4"),
            codec: match s.codec {
                AudioCodec::Mp4a => SimpleAudioCodec::Aac,
                AudioCodec::Opus => SimpleAudioCodec::Opus,
                _ => SimpleAudioCodec::Other,
            },
        })
        .collect();

    let duration_secs = if player.details.duration > 0 {
        Some(player.details.duration as u64)
    } else {
        None
    };

    let thumbnail_url = pick_thumbnail_url(&player.details.thumbnail, 480);

    CandidatePool {
        muxed,
        adaptive_video,
        audio,
        hls_url: player.hls_manifest_url,
        user_agent: ua.to_string(),
        duration_secs,
        title: player.details.name,
        channel_id: player.details.channel_id,
        channel_name: player.details.channel_name.unwrap_or_default(),
        thumbnail_url,
        view_count: player.details.view_count,
    }
}

fn map_rp_video_codec(c: VideoCodec) -> SimpleVideoCodec {
    match c {
        VideoCodec::Avc1 => SimpleVideoCodec::H264,
        VideoCodec::Vp9 => SimpleVideoCodec::Vp9,
        VideoCodec::Mp4v => SimpleVideoCodec::Mp4v,
        VideoCodec::Av01 => SimpleVideoCodec::Av1,
        _ => SimpleVideoCodec::Unknown,
    }
}

fn pick_best_video(streams: &[RawVideoCandidate], target_height: u32) -> Option<&RawVideoCandidate> {
    if streams.is_empty() {
        return None;
    }

    // Prefer streams at or below target_height; if all exceed target_height, pick lowest above it.
    let has_within_target = streams.iter().any(|s| s.height > 0 && s.height <= target_height);

    streams.iter().max_by(|a, b| {
        // 1. Prefer SDR over HDR on modest displays/GPUs
        let sdr_cmp = (!a.hdr).cmp(&(!b.hdr));
        if sdr_cmp != std::cmp::Ordering::Equal {
            return sdr_cmp;
        }

        if has_within_target {
            let a_ok = a.height > 0 && a.height <= target_height;
            let b_ok = b.height > 0 && b.height <= target_height;
            if a_ok != b_ok {
                return a_ok.cmp(&b_ok);
            }
            // Closest height up to target_height
            if a.height != b.height {
                return a.height.cmp(&b.height);
            }
        } else if a.height != b.height {
            // All streams exceed target: pick smallest height first
            return b.height.cmp(&a.height);
        }

        // 2. Modest-hardware codec priority: H.264 > VP9 > MP4V > AV1
        let codec_cmp = a.codec.hardware_rank().cmp(&b.codec.hardware_rank());
        if codec_cmp != std::cmp::Ordering::Equal {
            return codec_cmp;
        }

        // 3. Reasonable FPS (<= 60) & bitrate
        (a.fps <= 60)
            .cmp(&(b.fps <= 60))
            .then_with(|| a.bitrate.cmp(&b.bitrate))
    })
}

fn pick_best_audio(streams: &[RawAudioCandidate]) -> Option<&RawAudioCandidate> {
    streams.iter().max_by(|a, b| {
        let codec_rank = |c: SimpleAudioCodec| match c {
            SimpleAudioCodec::Aac => 3u8,
            SimpleAudioCodec::Opus => 2u8,
            SimpleAudioCodec::Other => 1u8,
        };
        // Prefer standard ~128-160 kbps AAC/Opus over ultra-low 48kbps
        let a_good = (96_000..=192_000).contains(&a.bitrate);
        let b_good = (96_000..=192_000).contains(&b.bitrate);
        a_good
            .cmp(&b_good)
            .then_with(|| codec_rank(a.codec).cmp(&codec_rank(b.codec)))
            .then_with(|| a.bitrate.cmp(&b.bitrate))
    })
}

fn select_best_playback(pool: &CandidatePool, quality: Quality) -> Option<ResolvedPlayback> {
    let ua = pool.user_agent.clone();
    let duration_secs = pool.duration_secs;

    if quality == Quality::AudioOnly {
        if let Some(aud) = pick_best_audio(&pool.audio) {
            return Some(ResolvedPlayback {
                primary_url: aud.url.clone(),
                audio_url: None,
                user_agent: ua,
                is_audio_only: true,
                duration_secs,
            });
        }
        if let Some(muxed) = pick_best_video(&pool.muxed, 360) {
            return Some(ResolvedPlayback {
                primary_url: muxed.url.clone(),
                audio_url: None,
                user_agent: ua,
                is_audio_only: true,
                duration_secs,
            });
        }
        if let Some(ref hls) = pool.hls_url {
            return Some(ResolvedPlayback {
                primary_url: hls.clone(),
                audio_url: None,
                user_agent: ua,
                is_audio_only: true,
                duration_secs,
            });
        }
        return None;
    }

    let target_h = quality.target_height().unwrap_or(360);

    // For 360p: progressive muxed MP4 (single HTTP connection, H.264 + AAC) opens fastest on modest hardware!
    if target_h <= 360 {
        if let Some(muxed) = pick_best_video(&pool.muxed, 480) {
            return Some(ResolvedPlayback {
                primary_url: muxed.url.clone(),
                audio_url: None,
                user_agent: ua,
                is_audio_only: false,
                duration_secs,
            });
        }
    }

    // Adaptive video + audio (720p / 1080p, or 360p when progressive muxed is absent)
    if let (Some(vid), Some(aud)) = (
        pick_best_video(&pool.adaptive_video, target_h),
        pick_best_audio(&pool.audio),
    ) {
        let muxed_match = pick_best_video(&pool.muxed, target_h);
        if let Some(m) = muxed_match {
            if m.height >= vid.height && m.codec.hardware_rank() >= vid.codec.hardware_rank() {
                return Some(ResolvedPlayback {
                    primary_url: m.url.clone(),
                    audio_url: None,
                    user_agent: ua,
                    is_audio_only: false,
                    duration_secs,
                });
            }
        }
        return Some(ResolvedPlayback {
            primary_url: vid.url.clone(),
            audio_url: Some(aud.url.clone()),
            user_agent: ua,
            is_audio_only: false,
            duration_secs,
        });
    }

    // Fallback to progressive muxed or HLS manifest (e.g. livestreams)
    if let Some(muxed) = pick_best_video(&pool.muxed, target_h) {
        return Some(ResolvedPlayback {
            primary_url: muxed.url.clone(),
            audio_url: None,
            user_agent: ua,
            is_audio_only: false,
            duration_secs,
        });
    }

    if let Some(ref hls) = pool.hls_url {
        return Some(ResolvedPlayback {
            primary_url: hls.clone(),
            audio_url: None,
            user_agent: ua,
            is_audio_only: false,
            duration_secs,
        });
    }

    None
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
    let normalize = |u: &str| {
        if let Some(stripped) = u.strip_prefix("//") {
            format!("https://{stripped}")
        } else {
            u.to_string()
        }
    };
    if let Some(found) = thumbs.iter().find(|t| t.width >= min_width) {
        return normalize(&found.url);
    }
    thumbs
        .last()
        .map(|t| normalize(&t.url))
        .unwrap_or_default()
}

fn map_youtube_item(item: YouTubeItem) -> SearchItem {
    match item {
        YouTubeItem::Video(v) => SearchItem::Video(map_video_item(v, None, false)),
        YouTubeItem::Channel(c) => SearchItem::Channel(map_channel_item(c)),
        YouTubeItem::Playlist(p) => SearchItem::Playlist(map_playlist_item(p)),
    }
}

fn map_channel_item(c: ChannelItem) -> ChannelSearchSummary {
    let avatar_url = pick_thumbnail_url(&c.avatar, 128);
    let handle = c
        .handle
        .unwrap_or_else(|| format!("@{}", c.name.to_lowercase().replace(' ', "")));
    let subscriber_count_text = c
        .subscriber_count
        .map(format_count)
        .map(|s| format!("{s} subscribers"))
        .unwrap_or_default();

    ChannelSearchSummary {
        id: c.id,
        name: c.name,
        handle,
        subscriber_count_text,
        description: c.short_description,
        avatar_url,
    }
}

fn map_playlist_item(p: PlaylistItem) -> PlaylistSearchSummary {
    let thumbnail_url = pick_thumbnail_url(&p.thumbnail, 320);
    let channel_name = p.channel.map(|ch| ch.name).unwrap_or_default();
    let video_count_text = p
        .video_count
        .map(|n| format!("{} videos", format_count(n)))
        .unwrap_or_else(|| "Playlist".to_string());

    PlaylistSearchSummary {
        id: p.id,
        name: p.name,
        channel_name,
        video_count_text,
        thumbnail_url,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_friendly_codec_selection() {
        let pool = CandidatePool {
            muxed: vec![RawVideoCandidate {
                url: "https://cdn.example/360_muxed.mp4".to_string(),
                height: 360,
                fps: 30,
                bitrate: 500_000,
                hdr: false,
                is_mp4: true,
                codec: SimpleVideoCodec::H264,
            }],
            adaptive_video: vec![
                RawVideoCandidate {
                    url: "https://cdn.example/720_av1.mp4".to_string(),
                    height: 720,
                    fps: 30,
                    bitrate: 900_000,
                    hdr: false,
                    is_mp4: true,
                    codec: SimpleVideoCodec::Av1,
                },
                RawVideoCandidate {
                    url: "https://cdn.example/720_h264.mp4".to_string(),
                    height: 720,
                    fps: 30,
                    bitrate: 1_200_000,
                    hdr: false,
                    is_mp4: true,
                    codec: SimpleVideoCodec::H264,
                },
            ],
            audio: vec![RawAudioCandidate {
                url: "https://cdn.example/128k.m4a".to_string(),
                bitrate: 128_000,
                is_m4a: true,
                codec: SimpleAudioCodec::Aac,
            }],
            hls_url: None,
            user_agent: "TestUA".to_string(),
            duration_secs: Some(180),
            title: None,
            channel_id: String::new(),
            channel_name: String::new(),
            thumbnail_url: String::new(),
            view_count: None,
        };

        // 360p prefers single-connection progressive muxed MP4
        let p360 = select_best_playback(&pool, Quality::P360).unwrap();
        assert_eq!(p360.primary_url, "https://cdn.example/360_muxed.mp4");
        assert!(p360.audio_url.is_none());

        // 720p prefers H.264 over AV1 and pairs adaptive audio
        let p720 = select_best_playback(&pool, Quality::P720).unwrap();
        assert_eq!(p720.primary_url, "https://cdn.example/720_h264.mp4");
        assert_eq!(
            p720.audio_url.as_deref(),
            Some("https://cdn.example/128k.m4a")
        );

        // AudioOnly picks direct audio stream
        let paud = select_best_playback(&pool, Quality::AudioOnly).unwrap();
        assert!(paud.is_audio_only);
        assert_eq!(paud.primary_url, "https://cdn.example/128k.m4a");
        assert!(paud.audio_url.is_none());
    }
}
