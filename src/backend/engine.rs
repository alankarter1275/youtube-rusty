use super::models::{
    format_count, format_duration, format_relative_published_text, relative_published_timestamp,
    ChannelDetails, ChannelSearchSummary, ChannelSortOrder, DurationFilter, PlaylistSearchSummary,
    SearchItem, SearchSortOrder, SearchTab, UploadDateFilter, VideoCollaborator, VideoSummary,
};
use super::subscriptions::SubscribedChannel;
use crate::player::mpv::{Quality, ResolvedPlayback};
use rustypipe::client::{ClientType, RustyPipe, RustyPipeQuery};
use rustypipe::model::paginator::{ContinuationEndpoint, Paginator};
use rustypipe::model::{
    AudioCodec, ChannelItem, PlaylistItem, Thumbnail, UrlTarget, VideoCodec, VideoItem,
    VideoPlayer, YouTubeItem,
};
use rustypipe::param::search_filter::{ItemType, Length, Order, SearchFilter, UploadDate};
use rustypipe::param::{ChannelOrder, ChannelVideoTab};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;

const VISIONOS_APP_VERSION: &str = "1.04";
const VISIONOS_APP_UA: &str =
    "com.google.visionos.youtube/1.04(RealityDevice17,1; U; CPU visionOS 26_6_0 like Mac OS X; US)";

const VISIONOS_SAFARI_VERSION: &str = "1.02";
const VISIONOS_SAFARI_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

const RP_TV_UA: &str = "Mozilla/5.0 (SMART-TV; Linux; Tizen 5.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/5.0 NativeTVAds Safari/538.1";
const RP_DESKTOP_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const YT_WEB_REFERER: &str = "https://www.youtube.com/";

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn random_innertube_token(len: usize) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15);
    let seq = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = nanos ^ seq.wrapping_mul(0x9e3779b97f4a7c15) ^ (std::process::id() as u64);
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        out.push(ALPHABET[(z & 63) as usize] as char);
    }
    out
}

fn append_cpn_param(url: &str, cpn: &str) -> String {
    if cpn.is_empty() || url.contains("&cpn=") || url.contains("?cpn=") || url.contains("/cpn/") {
        return url.to_string();
    }
    if url.contains('?') {
        format!("{url}&cpn={cpn}")
    } else {
        format!("{url}?cpn={cpn}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    RustyPipe,
    DirectChannelShorts,
    DirectPlaylistVideos,
    DirectHomeFeed,
    DirectSearch,
    DirectChannelPlaylists,
}

#[derive(Clone, Debug)]
pub struct PageCursor {
    pub ctoken: String,
    pub endpoint: ContinuationEndpoint,
    pub visitor_data: Option<String>,
    pub kind: CursorKind,
}

#[derive(Clone, Debug)]
pub struct PagedPlaylists {
    pub items: Vec<PlaylistSearchSummary>,
    pub next: Option<PageCursor>,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelHeaderStats {
    pub channel_name: Option<String>,
    pub handle: Option<String>,
    pub subscriber_count_text: Option<String>,
    pub video_count_text: Option<String>,
    pub avatar_url: Option<String>,
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
    itag: u32,
    url: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    filesize: Option<u64>,
    hdr: bool,
    is_mp4: bool,
    codec: SimpleVideoCodec,
}

#[derive(Clone, Debug)]
struct RawAudioCandidate {
    itag: u32,
    url: String,
    bitrate: u32,
    sample_rate: u32,
    filesize: Option<u64>,
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
    referer: String,
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

#[derive(Clone, Debug)]
struct VerifiedPoolEntry {
    video_id: String,
    fetched_at: Instant,
    pool: CandidatePool,
    hls_manifest: Option<String>,
}

const VERIFIED_POOL_TTL: Duration = Duration::from_secs(720);
const VERIFIED_POOL_CAP: usize = 24;

fn cursor_from_paginator<T>(p: &Paginator<T>) -> Option<PageCursor> {
    p.ctoken.as_ref().map(|ctoken| PageCursor {
        ctoken: ctoken.clone(),
        endpoint: p.endpoint,
        visitor_data: p.visitor_data.clone(),
        kind: CursorKind::RustyPipe,
    })
}

const VISITOR_DATA_TTL: Duration = Duration::from_secs(45 * 60);

/// Synthesizes a valid YouTube protobuf `visitorData` token (`field 1 = 11-char visitor id`,
/// `field 5 = unix timestamp`) so `RustyPipe` never falls back to its internal `VisitorDataCache`
/// (`rustypipe 0.11.4` has an upstream `.unwrap()` on `https://music.youtube.com/`).
fn synthesize_visitor_data() -> String {
    let id = random_innertube_token(11);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_730_000_000);
    let mut proto = Vec::with_capacity(24);
    push_proto_bytes_field(&mut proto, 1, id.as_bytes());
    push_proto_varint_field(&mut proto, 5, now_secs);
    encode_base64_url_safe_pct(&proto)
}

pub struct Engine {
    rp: RustyPipe,
    http: reqwest::Client,
    visitor_data: RwLock<Option<(String, Instant)>>,
    visitor_fetch_lock: Mutex<()>,
    verified_pools: RwLock<Vec<VerifiedPoolEntry>>,
    pool_inflight: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    home_seed: AtomicU64,
    seen_home_ids: RwLock<Vec<String>>,
    category_seed: AtomicU64,
    seen_category_ids: RwLock<Vec<String>>,
}

impl Engine {
    pub fn new(storage_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(storage_dir);
        let boot_vd = synthesize_visitor_data();
        let rp = RustyPipe::builder()
            .storage_dir(storage_dir)
            .no_botguard()
            .no_reporter()
            .visitor_data(boot_vd)
            .build()
            .unwrap_or_else(|_| {
                RustyPipe::builder()
                    .no_storage()
                    .no_botguard()
                    .no_reporter()
                    .visitor_data(synthesize_visitor_data())
                    .build()
                    .expect("RustyPipe client init")
            });
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let init_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
            .unwrap_or(7);
        Self {
            rp,
            http,
            visitor_data: RwLock::new(None),
            visitor_fetch_lock: Mutex::new(()),
            verified_pools: RwLock::new(Vec::new()),
            pool_inflight: Mutex::new(HashMap::new()),
            home_seed: AtomicU64::new(init_seed),
            seen_home_ids: RwLock::new(Vec::new()),
            category_seed: AtomicU64::new(init_seed.wrapping_add(3)),
            seen_category_ids: RwLock::new(Vec::new()),
        }
    }

    pub fn http_client(&self) -> reqwest::Client {
        self.http.clone()
    }

    pub async fn warm_startup(&self) {
        let _ = self.ensure_visitor_data().await;
    }

    /// Returns a `RustyPipeQuery` guaranteed to carry an explicit `visitor_data` token so
    /// `rustypipe 0.11.4` never enters its internal `VisitorDataCache::new_visitor_data()`
    /// (which panics on `.unwrap()` if `https://music.youtube.com/` fails).
    async fn rp_query(&self) -> RustyPipeQuery {
        let vd = self
            .ensure_visitor_data()
            .await
            .unwrap_or_else(synthesize_visitor_data);
        self.rp.query().visitor_data(vd)
    }

    async fn ensure_visitor_data(&self) -> Option<String> {
        {
            let guard = self.visitor_data.read().await;
            if let Some((ref vd, fetched_at)) = *guard {
                if fetched_at.elapsed() < VISITOR_DATA_TTL {
                    return Some(vd.clone());
                }
            }
        }

        let _fetch_guard = self.visitor_fetch_lock.lock().await;
        {
            let guard = self.visitor_data.read().await;
            if let Some((ref vd, fetched_at)) = *guard {
                if fetched_at.elapsed() < VISITOR_DATA_TTL {
                    return Some(vd.clone());
                }
            }
        }

        let body = json!({
            "context": {
                "client": {
                    "clientName": "VISIONOS",
                    "clientVersion": VISIONOS_APP_VERSION,
                    "clientScreen": "WATCH",
                    "platform": "MOBILE",
                    "deviceMake": "Apple",
                    "deviceModel": "RealityDevice17,1",
                    "osName": "visionOS",
                    "osVersion": "26.6.0.23O770",
                    "hl": "en",
                    "gl": "US",
                    "utcOffsetMinutes": 0
                }
            }
        });

        let fetched = match self
            .http
            .post("https://www.youtube.com/youtubei/v1/visitor_id?prettyPrint=false")
            .timeout(Duration::from_secs(5))
            .header("User-Agent", VISIONOS_APP_UA)
            .header("Content-Type", "application/json")
            .header("X-Goog-Api-Format-Version", "2")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| {
                    v.pointer("/responseContext/visitorData")
                        .and_then(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                }),
            Err(_) => None,
        };

        if let Some(ref vd) = fetched {
            *self.visitor_data.write().await = Some((vd.clone(), Instant::now()));
            return Some(vd.clone());
        }

        // Fall back to stale cached visitor_data if YouTube had a transient network blip
        self.visitor_data
            .read()
            .await
            .as_ref()
            .map(|(vd, _)| vd.clone())
    }

    /// Resolves direct YouTube media stream URLs in pure Rust without `yt-dlp` or Python,
    /// populating a `quality-menu.lua`-compatible `ytdl_json` format catalog and honoring
    /// optional per-file `ytdl-format` selections (e.g. `v247+a251`, `hls-720`).
    /// Verified stream pools are cached in memory for 12 minutes so mid-video resolution switches
    /// and 1-ahead track transitions complete in 0ms without re-querying Innertube.
    pub async fn resolve_playback_for_mpv(
        &self,
        video_id: &str,
        default_quality: Quality,
        ytdl_format_override: Option<&str>,
    ) -> Result<ResolvedPlayback, String> {
        let entry = self.fetch_or_get_verified_pool(video_id).await?;
        resolve_from_verified_entry(&entry, default_quality, ytdl_format_override)
            .ok_or_else(|| "No playable streams available for this video.".to_string())
    }

    /// Warms the verified stream pool for `video_id` in background so upcoming queue transitions
    /// open instantaneously when MPV advances.
    pub async fn prefetch_video_pool(&self, video_id: &str) {
        if !video_id.is_empty() {
            let _ = self.fetch_or_get_verified_pool(video_id).await;
        }
    }

    async fn fetch_or_get_verified_pool(&self, video_id: &str) -> Result<VerifiedPoolEntry, String> {
        {
            let guard = self.verified_pools.read().await;
            if let Some(found) = guard
                .iter()
                .find(|e| e.video_id == video_id && e.fetched_at.elapsed() < VERIFIED_POOL_TTL)
            {
                return Ok(found.clone());
            }
        }

        // Coalesce concurrent requests for the same video_id (e.g. parallel startup prefetch + MPV hook)
        let vid_lock = {
            let mut inflight = self.pool_inflight.lock().await;
            if inflight.len() > 16 {
                inflight.retain(|_, v| Arc::strong_count(v) > 1);
            }
            inflight
                .entry(video_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _vid_guard = vid_lock.lock().await;

        {
            let guard = self.verified_pools.read().await;
            if let Some(found) = guard
                .iter()
                .find(|e| e.video_id == video_id && e.fetched_at.elapsed() < VERIFIED_POOL_TTL)
            {
                return Ok(found.clone());
            }
        }

        let mut last_err = String::new();
        let mut sd_fallback: Option<VerifiedPoolEntry> = None;

        // 1. VISIONOS App (NewPipe 2026.11 spec: POT-free & JS-free direct MP4 + full HLS ladder)
        match self.fetch_innertube_visionos_app(video_id).await {
            Ok(pool) if pool.has_playable_streams() => {
                if let Some(entry) = self.probe_pool_into_verified(video_id, pool).await {
                    if verified_entry_has_hd(&entry) {
                        self.store_verified_pool(entry.clone()).await;
                        return Ok(entry);
                    }
                    sd_fallback = Some(entry);
                } else {
                    last_err = "VisionOS app streams returned HTTP 403".to_string();
                }
            }
            Ok(_) => {}
            Err(e) => last_err = e,
        }

        // 2. VISIONOS Safari (yt-dlp 2026.10 `_DEFAULT_JSLESS_CLIENTS` spec)
        match self.fetch_innertube_visionos_safari(video_id).await {
            Ok(pool) if pool.has_playable_streams() => {
                if let Some(entry) = self.probe_pool_into_verified(video_id, pool).await {
                    if verified_entry_has_hd(&entry) {
                        self.store_verified_pool(entry.clone()).await;
                        return Ok(entry);
                    }
                    if sd_fallback.is_none() {
                        sd_fallback = Some(entry);
                    }
                } else {
                    last_err = "VisionOS Safari streams returned HTTP 403".to_string();
                }
            }
            Ok(_) => {}
            Err(e) => last_err = e,
        }

        // 3. RustyPipe pure-Rust JS player deobfuscation (TV & Desktop — covers "Made for Kids" videos too)
        for (ctype, ua) in [
            (ClientType::Tv, RP_TV_UA),
            (ClientType::Desktop, RP_DESKTOP_UA),
        ] {
            match self.rp_query().await.player_from_clients(video_id, &[ctype]).await {
                Ok(player) => {
                    let pool = pool_from_rustypipe(player, ua, YT_WEB_REFERER);
                    if pool.has_playable_streams() {
                        if let Some(entry) = self.probe_pool_into_verified(video_id, pool).await {
                            self.store_verified_pool(entry.clone()).await;
                            return Ok(entry);
                        }
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
        }

        if let Some(entry) = sd_fallback {
            self.store_verified_pool(entry.clone()).await;
            return Ok(entry);
        }

        if last_err.is_empty() {
            Err("No playable streams available for this video.".to_string())
        } else {
            Err(format!("Stream extraction failed: {last_err}"))
        }
    }

    async fn store_verified_pool(&self, entry: VerifiedPoolEntry) {
        let mut guard = self.verified_pools.write().await;
        guard.retain(|e| {
            e.video_id != entry.video_id && e.fetched_at.elapsed() < VERIFIED_POOL_TTL
        });
        if guard.len() >= VERIFIED_POOL_CAP {
            guard.remove(0);
        }
        guard.push(entry);
    }

    async fn probe_pool_into_verified(
        &self,
        video_id: &str,
        mut pool: CandidatePool,
    ) -> Option<VerifiedPoolEntry> {
        let ua = pool.user_agent.clone();
        let referer = pool.referer.clone();
        let muxed_url = pool.muxed.first().map(|m| m.url.clone());
        let adap_url = pool.adaptive_video.first().map(|v| v.url.clone());
        let aud_url = pool.audio.first().map(|a| a.url.clone());
        let hls_url = pool.hls_url.clone();

        let (muxed_ok, adap_ok, aud_ok, hls_manifest) = tokio::join!(
            async {
                match muxed_url {
                    Some(ref u) => self.probe_media_url(u, &ua, &referer).await,
                    None => false,
                }
            },
            async {
                match adap_url {
                    Some(ref u) => self.probe_media_url(u, &ua, &referer).await,
                    None => false,
                }
            },
            async {
                match aud_url {
                    Some(ref u) => self.probe_media_url(u, &ua, &referer).await,
                    None => false,
                }
            },
            async {
                match hls_url {
                    Some(ref u) => self.fetch_text_checked(u, &ua, &referer).await,
                    None => None,
                }
            }
        );

        if !muxed_ok {
            pool.muxed.clear();
        }
        if !(adap_ok && aud_ok) {
            pool.adaptive_video.clear();
        }
        if !aud_ok {
            pool.audio.clear();
        }
        if hls_manifest.is_none() {
            pool.hls_url = None;
        }

        let has_playable = !pool.muxed.is_empty()
            || (!pool.adaptive_video.is_empty() && !pool.audio.is_empty())
            || !pool.audio.is_empty()
            || hls_manifest.is_some();

        if has_playable {
            Some(VerifiedPoolEntry {
                video_id: video_id.to_string(),
                fetched_at: Instant::now(),
                pool,
                hls_manifest,
            })
        } else {
            None
        }
    }

    async fn fetch_candidate_pool(&self, video_id: &str) -> Result<CandidatePool, String> {
        let mut last_err = String::new();

        match self.fetch_innertube_visionos_app(video_id).await {
            Ok(pool) if pool.has_playable_streams() => {
                if self.pool_has_reachable_media(&pool).await {
                    return Ok(pool);
                }
            }
            Ok(_) => {}
            Err(e) => last_err = e,
        }

        match self.fetch_innertube_visionos_safari(video_id).await {
            Ok(pool) if pool.has_playable_streams() => {
                if self.pool_has_reachable_media(&pool).await {
                    return Ok(pool);
                }
            }
            Ok(_) => {}
            Err(e) => last_err = e,
        }

        for (ctype, ua) in [
            (ClientType::Tv, RP_TV_UA),
            (ClientType::Desktop, RP_DESKTOP_UA),
        ] {
            match self.rp_query().await.player_from_clients(video_id, &[ctype]).await {
                Ok(player) => {
                    let pool = pool_from_rustypipe(player, ua, YT_WEB_REFERER);
                    if pool.has_playable_streams() && self.pool_has_reachable_media(&pool).await {
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

    async fn pool_has_reachable_media(&self, pool: &CandidatePool) -> bool {
        if let Some(m) = pool.muxed.first() {
            if self
                .probe_media_url(&m.url, &pool.user_agent, &pool.referer)
                .await
            {
                return true;
            }
        }
        if let Some(a) = pool.audio.first() {
            if self
                .probe_media_url(&a.url, &pool.user_agent, &pool.referer)
                .await
            {
                return true;
            }
        }
        if let Some(ref hls) = pool.hls_url {
            if self
                .fetch_text_checked(hls, &pool.user_agent, &pool.referer)
                .await
                .is_some()
            {
                return true;
            }
        }
        false
    }

    async fn probe_media_url(&self, url: &str, ua: &str, referer: &str) -> bool {
        let mut req = self
            .http
            .get(url)
            .timeout(Duration::from_millis(3500))
            .header("User-Agent", ua)
            .header("Range", "bytes=0-1");
        if !referer.is_empty() {
            req = req.header("Referer", referer);
        }
        match req.send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    async fn fetch_text_checked(&self, url: &str, ua: &str, referer: &str) -> Option<String> {
        let mut req = self
            .http
            .get(url)
            .timeout(Duration::from_millis(3500))
            .header("User-Agent", ua);
        if !referer.is_empty() {
            req = req.header("Referer", referer);
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.text().await.ok()
    }

    async fn resolve_hls_playback(
        &self,
        hls_master_url: &str,
        pool: &CandidatePool,
        quality: Quality,
    ) -> Option<ResolvedPlayback> {
        let manifest = self
            .fetch_text_checked(hls_master_url, &pool.user_agent, &pool.referer)
            .await?;

        if let Some((primary_url, audio_url)) = parse_hls_master_playlist(&manifest, quality) {
            return Some(ResolvedPlayback {
                primary_url,
                audio_url,
                user_agent: pool.user_agent.clone(),
                referer: pool.referer.clone(),
                is_audio_only: quality == Quality::AudioOnly,
                duration_secs: pool.duration_secs,
                title: pool.title.clone().unwrap_or_default(),
                ytdl_json: None,
            });
        }

        Some(ResolvedPlayback {
            primary_url: hls_master_url.to_string(),
            audio_url: None,
            user_agent: pool.user_agent.clone(),
            referer: pool.referer.clone(),
            is_audio_only: quality == Quality::AudioOnly,
            duration_secs: pool.duration_secs,
            title: pool.title.clone().unwrap_or_default(),
            ytdl_json: None,
        })
    }

    async fn fetch_innertube_visionos_app(&self, video_id: &str) -> Result<CandidatePool, String> {
        let visitor_data = self.ensure_visitor_data().await;
        let cpn = random_innertube_token(16);
        let t_param = random_innertube_token(12);

        let mut client_obj = json!({
            "clientName": "VISIONOS",
            "clientVersion": VISIONOS_APP_VERSION,
            "clientScreen": "WATCH",
            "platform": "MOBILE",
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "osName": "visionOS",
            "osVersion": "26.6.0.23O770",
            "hl": "en",
            "gl": "US",
            "utcOffsetMinutes": 0
        });
        if let (Some(ref vd), Some(map)) = (&visitor_data, client_obj.as_object_mut()) {
            map.insert("visitorData".to_string(), Value::String(vd.clone()));
        }

        let body = json!({
            "videoId": video_id,
            "cpn": cpn,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": client_obj
            }
        });

        let endpoint = format!(
            "https://youtubei.googleapis.com/youtubei/v1/player?prettyPrint=false&t={t_param}&id={video_id}"
        );
        self.post_innertube_player(
            &endpoint,
            video_id,
            &body,
            VISIONOS_APP_UA,
            "101",
            visitor_data.as_deref(),
            &cpn,
        )
        .await
    }

    async fn fetch_innertube_visionos_safari(
        &self,
        video_id: &str,
    ) -> Result<CandidatePool, String> {
        let visitor_data = self.ensure_visitor_data().await;
        let cpn = random_innertube_token(16);

        let mut client_obj = json!({
            "clientName": "VISIONOS",
            "clientVersion": VISIONOS_SAFARI_VERSION,
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "userAgent": VISIONOS_SAFARI_UA,
            "osName": "visionOS",
            "osVersion": "26.5.23O471",
            "hl": "en",
            "gl": "US",
            "utcOffsetMinutes": 0
        });
        if let (Some(ref vd), Some(map)) = (&visitor_data, client_obj.as_object_mut()) {
            map.insert("visitorData".to_string(), Value::String(vd.clone()));
        }

        let body = json!({
            "videoId": video_id,
            "cpn": cpn,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": client_obj
            }
        });

        self.post_innertube_player(
            "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
            video_id,
            &body,
            VISIONOS_SAFARI_UA,
            "101",
            visitor_data.as_deref(),
            &cpn,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn post_innertube_player(
        &self,
        endpoint: &str,
        expected_video_id: &str,
        body: &Value,
        ua: &str,
        client_name_id: &str,
        visitor_data: Option<&str>,
        cpn: &str,
    ) -> Result<CandidatePool, String> {
        let client_ver = body
            .pointer("/context/client/clientVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("1.0");

        let mut req = self
            .http
            .post(endpoint)
            .header("User-Agent", ua)
            .header("Content-Type", "application/json")
            .header("X-Goog-Api-Format-Version", "2")
            .header("X-YouTube-Client-Name", client_name_id)
            .header("X-YouTube-Client-Version", client_ver);

        if let Some(vd) = visitor_data {
            req = req.header("X-Goog-Visitor-Id", vd);
        }

        let resp = req
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

        if let Some(returned_id) = val.pointer("/videoDetails/videoId").and_then(|s| s.as_str()) {
            if !returned_id.is_empty() && returned_id != expected_video_id {
                return Err("YouTube returned a fallback/mismatch video response".to_string());
            }
        }

        // Capture fresh visitorData if returned in responseContext
        if visitor_data.is_none() {
            if let Some(vd) = val
                .pointer("/responseContext/visitorData")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                *self.visitor_data.write().await = Some((vd.to_string(), Instant::now()));
            }
        }

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
                if let Some(mut vc) = parse_json_video_stream(fmt) {
                    vc.url = append_cpn_param(&vc.url, cpn);
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
                    if let Some(mut vc) = parse_json_video_stream(fmt) {
                        vc.url = append_cpn_param(&vc.url, cpn);
                        adaptive_video.push(vc);
                    }
                } else if mime.starts_with("audio/") {
                    if let Some(mut ac) = parse_json_audio_stream(fmt) {
                        ac.url = append_cpn_param(&ac.url, cpn);
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
            referer: String::new(),
            duration_secs,
            title,
            channel_id,
            channel_name,
            thumbnail_url,
            view_count,
        })
    }

    /// Pure-Rust downloader using verified `VISIONOS`/`RustyPipe` stream resolution + `reqwest` chunk streaming.
    /// Zero `yt-dlp` or Python required. Uses `ffmpeg -c copy` only if available for HD adaptive/HLS streams.
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
            if let Some(best_aud) = pick_best_audio(&pool.audio) {
                let ext = if best_aud.is_m4a { "m4a" } else { "webm" };
                let out_path = downloads_dir.join(format!("{base_name} [{video_id}].{ext}"));
                if self
                    .stream_url_to_file(&best_aud.url, &pool.user_agent, &pool.referer, &out_path)
                    .await
                    .is_ok()
                {
                    return Ok(out_path);
                }
            }
            if let Some(ref hls_master) = pool.hls_url {
                if has_ffmpeg().await {
                    let out_path = downloads_dir.join(format!("{base_name} [{video_id}].m4a"));
                    if let Some(play) = self
                        .resolve_hls_playback(hls_master, &pool, Quality::AudioOnly)
                        .await
                    {
                        self.ffmpeg_remux_hls(&play, &out_path, true).await?;
                        return Ok(out_path);
                    }
                }
            }
            return Err("No downloadable audio stream found.".to_string());
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
                        self.stream_url_to_file(&v.url, &pool.user_agent, &pool.referer, &tmp_v)
                            .await?;
                        self.stream_url_to_file(&a.url, &pool.user_agent, &pool.referer, &tmp_a)
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
            if self
                .stream_url_to_file(&muxed.url, &pool.user_agent, &pool.referer, &out_path)
                .await
                .is_ok()
            {
                return Ok(out_path);
            }
        }

        // HLS fallback download via ffmpeg if direct HTTP streams were absent/blocked
        if let Some(ref hls_master) = pool.hls_url {
            if has_ffmpeg().await {
                if let Some(play) = self.resolve_hls_playback(hls_master, &pool, quality).await {
                    let out_path = downloads_dir.join(format!("{base_name} [{video_id}].mp4"));
                    self.ffmpeg_remux_hls(&play, &out_path, false).await?;
                    return Ok(out_path);
                }
            }
        }

        Err("No downloadable file stream available (install ffmpeg for HD adaptive downloads).".to_string())
    }

    async fn ffmpeg_remux_hls(
        &self,
        play: &ResolvedPlayback,
        out_path: &Path,
        audio_only: bool,
    ) -> Result<(), String> {
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.arg("-y").arg("-loglevel").arg("error");
        if !play.user_agent.is_empty() {
            cmd.arg("-user_agent").arg(&play.user_agent);
        }
        cmd.arg("-i").arg(&play.primary_url);
        if let Some(ref aud) = play.audio_url {
            if !audio_only {
                if !play.user_agent.is_empty() {
                    cmd.arg("-user_agent").arg(&play.user_agent);
                }
                cmd.arg("-i").arg(aud);
            }
        }
        if audio_only {
            cmd.arg("-vn");
        }
        cmd.arg("-c")
            .arg("copy")
            .arg(out_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let status = cmd
            .status()
            .await
            .map_err(|e| format!("ffmpeg error: {e}"))?;
        if !status.success() {
            return Err("ffmpeg could not save HLS stream".to_string());
        }
        Ok(())
    }

    async fn stream_url_to_file(
        &self,
        url: &str,
        user_agent: &str,
        referer: &str,
        dest: &Path,
    ) -> Result<(), String> {
        let mut req = self.http.get(url).header("User-Agent", user_agent);
        if !referer.is_empty() {
            req = req.header("Referer", referer);
        }
        let mut resp = req
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
        self.rp_query().await
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
                let direct_params = encode_direct_search_params(sort, date, duration, None);
                if let Ok(direct_all) = self
                    .search_direct(query, Some(&direct_params), DirectSearchMode::All)
                    .await
                {
                    if !direct_all.items.is_empty() {
                        return Ok(direct_all);
                    }
                }
                let paginator = if is_default_filters {
                    self.rp_query().await
                        .search::<YouTubeItem, _>(query)
                        .await
                        .map_err(|e| format!("Search error: {e}"))?
                        .items
                } else {
                    let filter = SearchFilter::new()
                        .date_opt(rp_date)
                        .length_opt(rp_len)
                        .sort_opt(rp_sort);
                    self.rp_query().await
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
            SearchTab::Shorts => {
                let shorts_params =
                    encode_direct_search_params(sort, date, DurationFilter::AnyLength, Some(9));
                if let Ok(direct_shorts) = self
                    .search_direct(query, Some(&shorts_params), DirectSearchMode::ShortsOnly)
                    .await
                {
                    if !direct_shorts.items.is_empty() {
                        return Ok(direct_shorts);
                    }
                }

                // Fallback 1: inspect YouTube's "Shorts" refinement chip in the search header
                if let Ok(raw_all) = self.fetch_search_json(Some(query), Some("8AEB"), None).await {
                    if let Some((chip_ctok, chip_params)) =
                        extract_search_header_shorts_chip(&raw_all)
                    {
                        if let Some(tok) = chip_ctok {
                            if let Ok(cont_page) = self
                                .continue_search_direct(&tok, DirectSearchMode::ShortsOnly)
                                .await
                            {
                                if !cont_page.items.is_empty() {
                                    return Ok(cont_page);
                                }
                            }
                        }
                        if let Some(p) = chip_params {
                            if let Ok(param_page) = self
                                .search_direct(query, Some(&p), DirectSearchMode::ShortsOnly)
                                .await
                            {
                                if !param_page.items.is_empty() {
                                    return Ok(param_page);
                                }
                            }
                        }
                    }
                    let extracted = parse_direct_search_response(&raw_all, DirectSearchMode::ShortsOnly);
                    if !extracted.items.is_empty() {
                        return Ok(extracted);
                    }
                }

                // Fallback 2: "#shorts" tagged search
                let tagged_q = format!("{query} #shorts");
                let fallback_params =
                    encode_direct_search_params(sort, date, DurationFilter::Under4Min, Some(1));
                self.search_direct(
                    &tagged_q,
                    Some(&fallback_params),
                    DirectSearchMode::ShortsOnly,
                )
                .await
            }
            SearchTab::Channels => {
                let filter = SearchFilter::new()
                    .item_type(ItemType::Channel)
                    .sort_opt(rp_sort);
                let paginator = self.rp_query().await
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
                let pl_params =
                    encode_direct_search_params(sort, date, DurationFilter::AnyLength, Some(3));
                self.search_direct(query, Some(&pl_params), DirectSearchMode::PlaylistsOnly)
                    .await
            }
        }
    }

    pub async fn load_more_search_items(
        &self,
        cursor: &PageCursor,
        tab: SearchTab,
    ) -> Result<PagedSearchItems, String> {
        if cursor.kind == CursorKind::DirectSearch {
            let mode = match tab {
                SearchTab::Shorts => DirectSearchMode::ShortsOnly,
                SearchTab::Playlists => DirectSearchMode::PlaylistsOnly,
                _ => DirectSearchMode::All,
            };
            return self.continue_search_direct(&cursor.ctoken, mode).await;
        }

        match tab {
            SearchTab::All => {
                let paginator = self.rp_query().await
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
            SearchTab::Videos | SearchTab::Shorts => {
                let is_shorts = tab == SearchTab::Shorts;
                let paged = self.load_more(cursor, None, is_shorts, false).await?;
                Ok(PagedSearchItems {
                    items: paged.items.into_iter().map(SearchItem::Video).collect(),
                    next: paged.next,
                })
            }
            SearchTab::Channels => {
                let paginator = self.rp_query().await
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
                let paginator = self.rp_query().await
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
        let clean_id = playlist_id
            .trim()
            .strip_prefix("VL")
            .unwrap_or_else(|| playlist_id.trim());
        if clean_id.is_empty() {
            return Err("Invalid playlist ID".to_string());
        }

        let browse_id = format!("VL{clean_id}");
        let mut header_only: Option<RemotePlaylist> = None;

        // 1. Plain `VL<id>` browse (supports standard playlists, Podcasts, Courses, and lockupViewModel items)
        let mut initial_val = self
            .fetch_channel_browse_json(&browse_id, None, None)
            .await
            .ok();
        if initial_val.is_none() {
            initial_val = self
                .fetch_channel_browse_json(&browse_id, Some("wgYCCAA="), None)
                .await
                .ok();
        }

        if let Some(val) = initial_val {
            if let Some(mut parsed) = parse_playlist_browse_response(&val, clean_id) {
                // 2. If initial browse returned header + lazy continuation token, resolve first page immediately
                if parsed.videos.is_empty() {
                    if let Some(ref next_cur) = parsed.next.clone() {
                        if let Ok(cont_page) =
                            self.continue_playlist_videos_direct(&next_cur.ctoken).await
                        {
                            parsed.videos = cont_page.items;
                            parsed.next = cont_page.next;
                        }
                    }
                }

                // 3. If still empty (e.g. YouTube's new lockup playlist interface omitting inline `contents`),
                // request the explicit initial PlaylistContinuation protobuf token on `/youtubei/v1/browse`
                if parsed.videos.is_empty() {
                    let synth_ctoken = build_initial_playlist_continuation_token(clean_id);
                    if let Ok(cont_val) = self
                        .fetch_channel_browse_json("", None, Some(&synth_ctoken))
                        .await
                    {
                        let cont_page = parse_playlist_continuation_response(&cont_val);
                        if !cont_page.items.is_empty() {
                            parsed.videos = cont_page.items;
                            parsed.next = cont_page.next;
                        }
                    }
                }

                // Backfill playlist owner name onto Podcast/Course episodes where YouTube omits redundant bylines
                if !parsed.channel_name.is_empty() {
                    for v in &mut parsed.videos {
                        if v.channel_name.is_empty() {
                            v.channel_name = parsed.channel_name.clone();
                        }
                    }
                }

                if !parsed.videos.is_empty() {
                    return Ok(parsed);
                }
                header_only = Some(parsed);
            }
        }

        // 4. Even if metadata browse failed, try synthetic initial continuation token directly
        if header_only.is_none() {
            let synth_ctoken = build_initial_playlist_continuation_token(clean_id);
            if let Ok(cont_val) = self
                .fetch_channel_browse_json("", None, Some(&synth_ctoken))
                .await
            {
                let cont_page = parse_playlist_continuation_response(&cont_val);
                if !cont_page.items.is_empty() {
                    let channel_name = cont_page
                        .items
                        .first()
                        .map(|v| v.channel_name.clone())
                        .unwrap_or_default();
                    let video_count_text = format!("{} videos", cont_page.items.len());
                    return Ok(RemotePlaylist {
                        id: clean_id.to_string(),
                        name: "Playlist".to_string(),
                        channel_name,
                        video_count_text,
                        videos: cont_page.items,
                        next: cont_page.next,
                    });
                }
            }
        }

        match self.rp_query().await.playlist(clean_id).await {
            Ok(pl) => {
                let channel_name = pl.channel.map(|c| c.name).unwrap_or_default();
                let video_count_text = if pl.video_count > 0 {
                    format!("{} videos", format_count(pl.video_count))
                } else {
                    format!("{} videos", pl.videos.items.len())
                };
                let next = cursor_from_paginator(&pl.videos);
                let videos: Vec<VideoSummary> = pl
                    .videos
                    .items
                    .into_iter()
                    .map(|it| map_video_item(it, None, false))
                    .collect();

                if videos.is_empty() {
                    if let Some(h) = header_only {
                        return Ok(h);
                    }
                }

                Ok(RemotePlaylist {
                    id: pl.id,
                    name: pl.name,
                    channel_name,
                    video_count_text,
                    videos,
                    next,
                })
            }
            Err(e) => {
                if let Some(h) = header_only {
                    Ok(h)
                } else {
                    Err(format!("Could not load playlist: {e}"))
                }
            }
        }
    }

    pub async fn channel_playlists(
        &self,
        channel_id: &str,
        fallback_channel_name: &str,
    ) -> Result<PagedPlaylists, String> {
        let val = self
            .fetch_channel_browse_json(channel_id, Some("EglwbGF5bGlzdHPyBgQKAkIA"), None)
            .await?;
        Ok(parse_channel_playlists_browse_response(
            &val,
            fallback_channel_name,
        ))
    }

    pub async fn continue_channel_playlists(
        &self,
        cursor: &PageCursor,
        fallback_channel_name: &str,
    ) -> Result<PagedPlaylists, String> {
        let val = self
            .fetch_channel_browse_json("", None, Some(&cursor.ctoken))
            .await?;
        Ok(parse_channel_playlists_browse_response(
            &val,
            fallback_channel_name,
        ))
    }

    async fn search_direct(
        &self,
        query: &str,
        params: Option<&str>,
        mode: DirectSearchMode,
    ) -> Result<PagedSearchItems, String> {
        let val = self.fetch_search_json(Some(query), params, None).await?;
        Ok(parse_direct_search_response(&val, mode))
    }

    async fn continue_search_direct(
        &self,
        ctoken: &str,
        mode: DirectSearchMode,
    ) -> Result<PagedSearchItems, String> {
        let val = self.fetch_search_json(None, None, Some(ctoken)).await?;
        Ok(parse_direct_search_response(&val, mode))
    }

    async fn fetch_search_json(
        &self,
        query: Option<&str>,
        params: Option<&str>,
        continuation: Option<&str>,
    ) -> Result<Value, String> {
        let mut body = serde_json::Map::new();
        body.insert(
            "context".to_string(),
            json!({
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20260728.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            }),
        );
        if let Some(ctoken) = continuation {
            body.insert("continuation".to_string(), Value::String(ctoken.to_string()));
        } else {
            if let Some(q) = query {
                body.insert("query".to_string(), Value::String(q.to_string()));
            }
            if let Some(p) = params {
                body.insert("params".to_string(), Value::String(p.to_string()));
            }
        }

        let resp = self
            .http
            .post("https://www.youtube.com/youtubei/v1/search?prettyPrint=false")
            .header("User-Agent", RP_DESKTOP_UA)
            .header("Origin", "https://www.youtube.com")
            .header("Referer", "https://www.youtube.com/")
            .header("Content-Type", "application/json")
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| format!("Search network error: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Search HTTP {}", resp.status()));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| format!("Search JSON error: {e}"))
    }

    async fn continue_playlist_videos_direct(&self, ctoken: &str) -> Result<PagedVideos, String> {
        let val = self
            .fetch_channel_browse_json("", None, Some(ctoken))
            .await?;
        Ok(parse_playlist_continuation_response(&val))
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

        let target = self.rp_query().await
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
            collaborators: Vec::new(),
                });
            }
        }

        let details = self.rp_query().await
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
            collaborators: Vec::new(),
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
            self.rp_query().await
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

            self.rp_query().await
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

    pub async fn category_feed(
        &self,
        category: &str,
        force_refresh: bool,
    ) -> Result<PagedVideos, String> {
        let variants: &[&str] = match category.to_ascii_lowercase().as_str() {
            "gaming" => &[
                "Gaming",
                "gaming highlights",
                "game review gameplay",
                "esports tournament",
                "indie games showcase",
            ],
            "music" => &[
                "Music",
                "live music session",
                "official music video",
                "acoustic performance",
                "concert live performance",
            ],
            "news" => &[
                "News",
                "world news analysis",
                "breaking news report",
                "global current affairs",
                "economics geopolitics news",
            ],
            _ => &[],
        };

        let step = if force_refresh {
            self.category_seed.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
        } else {
            self.category_seed.load(Ordering::Relaxed)
        };

        let chosen_query = if variants.is_empty() {
            category
        } else if !force_refresh {
            variants[0]
        } else {
            variants[(step as usize) % variants.len()]
        };

        let date_window = match step % 3 {
            1 if force_refresh => UploadDate::Day,
            2 if force_refresh => UploadDate::Month,
            _ => UploadDate::Week,
        };

        let direct_date = match &date_window {
            UploadDate::Day => UploadDateFilter::Today,
            UploadDate::Week => UploadDateFilter::ThisWeek,
            UploadDate::Month => UploadDateFilter::ThisMonth,
            UploadDate::Year => UploadDateFilter::ThisYear,
        };
        let filter = SearchFilter::new()
            .item_type(ItemType::Video)
            .date(date_window);
        let direct_params = encode_direct_search_params(
            SearchSortOrder::Relevance,
            direct_date,
            DurationFilter::AnyLength,
            Some(1),
        );

        // RustyPipe's VideoItem model drops YouTube's collaborator list. In parallel, inspect
        // the raw WEB search response and merge those channel credits onto the stable result list.
        let rustypipe_search = async {
            match self
                .rp_query()
                .await
                .search_filter::<VideoItem, _>(chosen_query, &filter)
                .await
            {
                Ok(res) if !res.items.items.is_empty() => Ok(res.items),
                _ => self
                    .rp_query()
                    .await
                    .search::<VideoItem, _>(chosen_query)
                    .await
                    .map(|result| result.items)
                    .map_err(|error| format!("Category error: {error}")),
            }
        };
        let (paginator, direct_json) = tokio::join!(
            rustypipe_search,
            tokio::time::timeout(
                Duration::from_secs(3),
                self.fetch_search_json(Some(chosen_query), Some(&direct_params), None),
            ),
        );
        let paginator = paginator?;

        let next = cursor_from_paginator(&paginator);
        let mut all_mapped: Vec<VideoSummary> = paginator
            .items
            .into_iter()
            .filter(|it| !it.is_short)
            .map(|it| map_video_item(it, None, false))
            .collect();
        if let Ok(Ok(raw)) = direct_json {
            let direct = parse_direct_search_response(&raw, DirectSearchMode::All);
            let direct_videos: Vec<VideoSummary> = direct
                .items
                .into_iter()
                .filter_map(|item| match item {
                    SearchItem::Video(video) => Some(video),
                    SearchItem::Channel(_) | SearchItem::Playlist(_) => None,
                })
                .collect();
            fill_missing_channel_metadata(&mut all_mapped, &direct_videos);
        }

        let prev_seen: HashSet<String> = if force_refresh {
            self.seen_category_ids.read().await.iter().cloned().collect()
        } else {
            HashSet::new()
        };

        let mut items: Vec<VideoSummary> = all_mapped
            .iter()
            .filter(|v| !prev_seen.contains(&v.id))
            .cloned()
            .collect();
        if items.len() < 8 {
            items = all_mapped;
        }
        if force_refresh && items.len() > 2 {
            shuffle_videos_seeded(&mut items, step ^ 0x9E37_79B9_7F4A_7C15);
        }

        {
            let mut guard = self.seen_category_ids.write().await;
            for v in items.iter().take(24) {
                guard.push(v.id.clone());
            }
            if guard.len() > 96 {
                let drain_n = guard.len() - 96;
                guard.drain(0..drain_n);
            }
        }

        Ok(PagedVideos { items, next })
    }

    pub async fn load_more(
        &self,
        cursor: &PageCursor,
        fallback_details: Option<&ChannelDetails>,
        force_short: bool,
        filter_out_shorts: bool,
    ) -> Result<PagedVideos, String> {
        match cursor.kind {
            CursorKind::DirectChannelShorts => {
                return self
                    .continue_channel_shorts_direct(&cursor.ctoken, fallback_details)
                    .await;
            }
            CursorKind::DirectPlaylistVideos | CursorKind::DirectHomeFeed => {
                let mut page = self.continue_playlist_videos_direct(&cursor.ctoken).await?;
                if cursor.kind == CursorKind::DirectHomeFeed {
                    if let Some(ref mut nc) = page.next {
                        nc.kind = CursorKind::DirectHomeFeed;
                    }
                    if filter_out_shorts {
                        page.items.retain(|v| !v.is_short);
                    }
                }
                return Ok(page);
            }
            CursorKind::DirectSearch => {
                let mode = if force_short {
                    DirectSearchMode::ShortsOnly
                } else {
                    DirectSearchMode::All
                };
                let page = self.continue_search_direct(&cursor.ctoken, mode).await?;
                let items = page
                    .items
                    .into_iter()
                    .filter_map(|it| match it {
                        SearchItem::Video(mut v) => {
                            if force_short {
                                v.is_short = true;
                                if v.duration_text.is_empty() {
                                    v.duration_text = "SHORTS".to_string();
                                }
                            }
                            if filter_out_shorts && v.is_short {
                                None
                            } else {
                                Some(v)
                            }
                        }
                        _ => None,
                    })
                    .collect();
                return Ok(PagedVideos {
                    items,
                    next: page.next,
                });
            }
            _ => {}
        }

        let paginator = self.rp_query().await
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

        let mut items: Vec<VideoSummary> = paginator
            .items
            .into_iter()
            .filter(|it| !filter_out_shorts || !it.is_short)
            .map(|it| map_video_item(it, fallback.as_ref(), force_short))
            .collect();

        // Channel continuation pages can use the same newer renderers that omit metadata.
        // Fill gaps from the WEB continuation response only for channel pages, not general feeds.
        if !force_short
            && fallback_details.is_some()
            && items.iter().any(|item| {
                !item.is_short
                    && (item.duration_seconds.is_none()
                        || item.duration_text.is_empty()
                        || (item.published_unix.is_none() && item.published_text.is_empty()))
            })
        {
            if let Ok(raw) = self
                .fetch_channel_browse_json("", None, Some(&cursor.ctoken))
                .await
            {
                let direct_page = parse_home_feed_response(&raw);
                fill_missing_channel_metadata(&mut items, &direct_page.items);
            }
        }

        Ok(PagedVideos { items, next })
    }

    /// Dynamic Home discovery feed: blends YouTube `FEwhat_to_watch` (when available) with a rotating
    /// 16-topic discovery pool and deduplicates recently displayed videos so Refresh (`Ctrl+R` / `↻`)
    /// always delivers a genuinely fresh page of videos.
    pub async fn trending_initial(&self, force_refresh: bool) -> Result<PagedTrending, String> {
        const HOME_DISCOVERY_TOPICS: &[&str] = &[
            "technology review",
            "science documentary",
            "podcast interview",
            "world news analysis",
            "software engineering",
            "cinema video essay",
            "history explained",
            "live music performance",
            "space astronomy",
            "gaming highlights",
            "economics finance",
            "hardware build",
            "standup comedy",
            "geopolitics documentary",
            "physics explained",
            "engineering documentary",
        ];

        if force_refresh {
            *self.visitor_data.write().await = None;
        }

        let step = self.home_seed.fetch_add(1, Ordering::Relaxed);
        let base_idx = ((step as usize) * 4) % HOME_DISCOVERY_TOPICS.len();
        let chosen_topics: [&str; 4] = [
            HOME_DISCOVERY_TOPICS[base_idx % HOME_DISCOVERY_TOPICS.len()],
            HOME_DISCOVERY_TOPICS[(base_idx + 1) % HOME_DISCOVERY_TOPICS.len()],
            HOME_DISCOVERY_TOPICS[(base_idx + 2) % HOME_DISCOVERY_TOPICS.len()],
            HOME_DISCOVERY_TOPICS[(base_idx + 3) % HOME_DISCOVERY_TOPICS.len()],
        ];

        let date_mode = step % 3;
        let base_query = self.rp_query().await;
        let mut join_set = JoinSet::new();

        for (idx, topic) in chosen_topics.into_iter().enumerate() {
            let rp_q = base_query.clone();
            let upload_date = match (date_mode + idx as u64) % 3 {
                0 => UploadDate::Week,
                1 => UploadDate::Day,
                _ => UploadDate::Month,
            };
            join_set.spawn(async move {
                let filter = SearchFilter::new()
                    .item_type(ItemType::Video)
                    .date(upload_date);
                let res = rp_q
                    .search_filter::<VideoItem, _>(topic, &filter)
                    .await
                    .map(|r| r.items)
                    .map_err(|e| e.to_string());
                (idx, res)
            });
        }

        let home_browse_fut = self.fetch_channel_browse_json("FEwhat_to_watch", None, None);
        let home_browse_res = tokio::time::timeout(Duration::from_secs(4), home_browse_fut).await;

        let mut buckets: Vec<Vec<VideoSummary>> = vec![Vec::new(); chosen_topics.len() + 1];
        let mut cursors = Vec::new();
        let mut last_err = None;

        if let Ok(Ok(home_json)) = home_browse_res {
            let home_page = parse_home_feed_response(&home_json);
            if !home_page.items.is_empty() {
                if let Some(c) = home_page.next {
                    cursors.push(c);
                }
                buckets[0] = home_page
                    .items
                    .into_iter()
                    .filter(|v| !v.is_short && v.duration_seconds.unwrap_or(120) >= 60)
                    .collect();
            }
        }

        while let Some(joined) = join_set.join_next().await {
            if let Ok((idx, res)) = joined {
                match res {
                    Ok(paginator) => {
                        if let Some(c) = cursor_from_paginator(&paginator) {
                            cursors.push(c);
                        }
                        buckets[idx + 1] = paginator
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

        let recently_seen: HashSet<String> = self.seen_home_ids.read().await.iter().cloned().collect();

        let mut fresh = Vec::new();
        let mut fallback_repeats = Vec::new();
        let mut seen = HashSet::new();
        let max_len = buckets.iter().map(|b| b.len()).max().unwrap_or(0);
        for i in 0..max_len {
            for bucket in &buckets {
                if let Some(v) = bucket.get(i) {
                    if seen.insert(v.id.clone()) {
                        if recently_seen.contains(&v.id) {
                            fallback_repeats.push(v.clone());
                        } else {
                            fresh.push(v.clone());
                        }
                    }
                }
            }
        }

        shuffle_videos_seeded(&mut fresh, step.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
        fresh.truncate(20);
        if fresh.len() < 16 {
            for rep in fallback_repeats {
                if fresh.len() >= 16 {
                    break;
                }
                fresh.push(rep);
            }
        }

        if !fresh.is_empty() {
            let mut guard = self.seen_home_ids.write().await;
            for v in &fresh {
                guard.push(v.id.clone());
            }
            if guard.len() > 120 {
                let drain_n = guard.len() - 120;
                guard.drain(0..drain_n);
            }
            return Ok(PagedTrending {
                items: fresh,
                cursors,
            });
        }

        let fallback = self.rp_query().await
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
        let ch = self.rp_query().await
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
        let mut items: Vec<VideoSummary> = ch
            .content
            .items
            .into_iter()
            .map(|it| map_video_item(it, Some(&fallback), false))
            .collect();

        // RustyPipe can omit duration or upload-date fields for newer renderer variants.
        // On that partial-data path, consult the WEB browse response for matching metadata.
        if items.iter().any(|item| {
            !item.is_short
                && (item.duration_seconds.is_none()
                    || item.duration_text.is_empty()
                    || (item.published_unix.is_none() && item.published_text.is_empty()))
        })
        {
            if let Ok(raw) = self
                .fetch_channel_browse_json(channel_id, Some("EgZ2aWRlb3PyBgQKAjoA"), None)
                .await
            {
                let direct_page = parse_home_feed_response(&raw);
                fill_missing_channel_metadata(&mut items, &direct_page.items);
            }
        }

        Ok((details, PagedVideos { items, next }))
    }

    pub async fn channel_shorts_with_stats(
        &self,
        channel_id: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<(Option<ChannelHeaderStats>, PagedVideos), String> {
        let val = self
            .fetch_channel_browse_json(channel_id, Some("EgZzaG9ydHPyBgUKA5oBAA%3D%3D"), None)
            .await?;

        let stats = extract_browse_header_stats(&val);
        let id_str = fallback_details
            .map(|d| d.id.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(channel_id);
        let name_str = stats
            .channel_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| fallback_details.map(|d| d.name.as_str()))
            .unwrap_or("");
        let av_str = stats
            .avatar_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| fallback_details.map(|d| d.avatar_url.as_str()))
            .unwrap_or("");

        let fb = ChannelFallback {
            id: id_str,
            name: name_str,
            avatar_url: av_str,
        };
        let paged = parse_shorts_browse_response(&val, id_str, Some(&fb));
        let stats_opt = if stats.subscriber_count_text.is_some()
            || stats.video_count_text.is_some()
            || stats.channel_name.is_some()
        {
            Some(stats)
        } else {
            None
        };
        Ok((stats_opt, paged))
    }

    pub async fn channel_shorts(
        &self,
        channel_id: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<PagedVideos, String> {
        let (_, paged) = self
            .channel_shorts_with_stats(channel_id, fallback_details)
            .await?;
        Ok(paged)
    }

    async fn continue_channel_shorts_direct(
        &self,
        ctoken: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<PagedVideos, String> {
        let val = self
            .fetch_channel_browse_json("", None, Some(ctoken))
            .await?;
        let cid = fallback_details.map(|d| d.id.as_str()).unwrap_or("");
        let fb = fallback_details.map(|d| ChannelFallback {
            id: &d.id,
            name: &d.name,
            avatar_url: &d.avatar_url,
        });
        Ok(parse_shorts_browse_response(&val, cid, fb.as_ref()))
    }

    async fn fetch_channel_browse_json(
        &self,
        channel_id: &str,
        params: Option<&str>,
        continuation: Option<&str>,
    ) -> Result<Value, String> {
        let mut body = serde_json::Map::new();
        body.insert(
            "context".to_string(),
            json!({
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20260728.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            }),
        );
        if let Some(ctoken) = continuation {
            body.insert("continuation".to_string(), Value::String(ctoken.to_string()));
        } else {
            body.insert("browseId".to_string(), Value::String(channel_id.to_string()));
            if let Some(p) = params {
                body.insert("params".to_string(), Value::String(p.to_string()));
            }
        }

        let resp = self
            .http
            .post("https://www.youtube.com/youtubei/v1/browse?prettyPrint=false")
            .header("User-Agent", RP_DESKTOP_UA)
            .header("Origin", "https://www.youtube.com")
            .header("Referer", "https://www.youtube.com/")
            .header("Content-Type", "application/json")
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| format!("Channel browse network error: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Channel browse HTTP {}", resp.status()));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| format!("Channel browse JSON error: {e}"))
    }

    pub async fn channel_tab_ordered(
        &self,
        channel_id: &str,
        is_shorts: bool,
        order: ChannelSortOrder,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<PagedVideos, String> {
        if order == ChannelSortOrder::Newest {
            if is_shorts {
                return self.channel_shorts(channel_id, fallback_details).await;
            }
            let (_, paged) = self.channel_videos(channel_id).await?;
            return Ok(paged);
        }

        // YouTube's sort controls carry opaque continuation tokens. Reuse the selected chip's
        // token directly instead of asking RustyPipe to synthesize one that the endpoint rejects.
        let wanted_label = match order {
            ChannelSortOrder::Popular => Some("popular"),
            ChannelSortOrder::Oldest => Some("oldest"),
            ChannelSortOrder::Newest => None,
        };
        if let Some(label) = wanted_label {
            let params = if is_shorts {
                "EgZzaG9ydHPyBgUKA5oBAA%3D%3D"
            } else {
                "EgZ2aWRlb3PyBgQKAjoA"
            };
            let tab_name = if is_shorts { "Shorts" } else { "Videos" };
            let raw = self
                .fetch_channel_browse_json(channel_id, Some(params), None)
                .await
                .map_err(|err| format!("Could not load {tab_name} sort options: {err}"))?;
            let token = extract_channel_sort_chip_token(&raw, label).or_else(|| {
                if order == ChannelSortOrder::Popular {
                    extract_channel_sort_chip_token(&raw, "most popular")
                } else {
                    None
                }
            });
            let token = token.ok_or_else(|| {
                format!("YouTube did not return a {tab_name} {label} sort token")
            })?;
            if is_shorts {
                return self
                    .continue_channel_shorts_direct(&token, fallback_details)
                    .await;
            }
            let mut page = self.continue_playlist_videos_direct(&token).await?;
            if let Some(next) = page.next.as_mut() {
                next.kind = CursorKind::DirectHomeFeed;
            }
            return Ok(page);
        }

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
        let paginator = self
            .rp_query()
            .await
            .channel_videos_tab_order(channel_id, tab, rp_order)
            .await
            .map_err(|e| format!("Channel sort error: {e}"))?;

        let fallback = fallback_details.map(|details| ChannelFallback {
            id: &details.id,
            name: &details.name,
            avatar_url: &details.avatar_url,
        });
        let next = cursor_from_paginator(&paginator);
        let items = paginator
            .items
            .into_iter()
            .map(|item| map_video_item(item, fallback.as_ref(), is_shorts))
            .collect();
        Ok(PagedVideos { items, next })
    }

    pub async fn channel_search(
        &self,
        channel_id: &str,
        query: &str,
        fallback_details: Option<&ChannelDetails>,
    ) -> Result<Vec<VideoSummary>, String> {
        let ch = self.rp_query().await
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
    let itag = fmt.get("itag").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let mime = fmt.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    let width = fmt.get("width").and_then(|w| w.as_u64()).unwrap_or(0) as u32;
    let height = fmt.get("height").and_then(|h| h.as_u64()).unwrap_or(0) as u32;
    let fps = fmt.get("fps").and_then(|f| f.as_u64()).unwrap_or(30) as u32;
    let bitrate = fmt
        .get("averageBitrate")
        .or_else(|| fmt.get("bitrate"))
        .and_then(|b| b.as_u64())
        .unwrap_or(0) as u32;
    let filesize = fmt
        .get("contentLength")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| v.as_u64())
        })
        .filter(|&sz| sz > 0);
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
        itag,
        url,
        width,
        height,
        fps,
        bitrate,
        filesize,
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
    let itag = fmt.get("itag").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let mime = fmt.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    let bitrate = fmt
        .get("averageBitrate")
        .or_else(|| fmt.get("bitrate"))
        .and_then(|b| b.as_u64())
        .unwrap_or(0) as u32;
    let filesize = fmt
        .get("contentLength")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| v.as_u64())
        })
        .filter(|&sz| sz > 0);
    let is_m4a = mime.starts_with("audio/mp4");
    let codec = if mime.contains("mp4a") {
        SimpleAudioCodec::Aac
    } else if mime.contains("opus") {
        SimpleAudioCodec::Opus
    } else {
        SimpleAudioCodec::Other
    };
    let default_sr = if codec == SimpleAudioCodec::Opus {
        48_000
    } else {
        44_100
    };
    let sample_rate = fmt
        .get("audioSampleRate")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| v.as_u64().map(|n| n as u32))
        })
        .filter(|&sr| sr > 0)
        .unwrap_or(default_sr);

    Some(RawAudioCandidate {
        itag,
        url,
        bitrate,
        sample_rate,
        filesize,
        is_m4a,
        codec,
    })
}

fn pool_from_rustypipe(player: VideoPlayer, ua: &str, referer: &str) -> CandidatePool {
    let muxed = player
        .video_streams
        .into_iter()
        .map(|s| RawVideoCandidate {
            itag: s.itag,
            url: s.url,
            width: s.width,
            height: s.height,
            fps: s.fps as u32,
            bitrate: if s.average_bitrate > 0 {
                s.average_bitrate
            } else {
                s.bitrate
            },
            filesize: s.size.filter(|&sz| sz > 0),
            hdr: s.hdr,
            is_mp4: s.mime.starts_with("video/mp4"),
            codec: map_rp_video_codec(s.codec),
        })
        .collect();

    let adaptive_video = player
        .video_only_streams
        .into_iter()
        .map(|s| RawVideoCandidate {
            itag: s.itag,
            url: s.url,
            width: s.width,
            height: s.height,
            fps: s.fps as u32,
            bitrate: if s.average_bitrate > 0 {
                s.average_bitrate
            } else {
                s.bitrate
            },
            filesize: s.size.filter(|&sz| sz > 0),
            hdr: s.hdr,
            is_mp4: s.mime.starts_with("video/mp4"),
            codec: map_rp_video_codec(s.codec),
        })
        .collect();

    let audio = player
        .audio_streams
        .into_iter()
        .map(|s| {
            let codec = match s.codec {
                AudioCodec::Mp4a => SimpleAudioCodec::Aac,
                AudioCodec::Opus => SimpleAudioCodec::Opus,
                _ => SimpleAudioCodec::Other,
            };
            let sample_rate = if codec == SimpleAudioCodec::Opus {
                48_000
            } else {
                44_100
            };
            RawAudioCandidate {
                itag: s.itag,
                url: s.url,
                bitrate: if s.average_bitrate > 0 {
                    s.average_bitrate
                } else {
                    s.bitrate
                },
                sample_rate,
                filesize: Some(s.size).filter(|&sz| sz > 0),
                is_m4a: s.mime.starts_with("audio/mp4"),
                codec,
            }
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
        referer: referer.to_string(),
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

    // Prefer streams at or below target_height (using short edge on portrait Shorts);
    // if all exceed target_height, pick lowest above it.
    let has_within_target = streams.iter().any(|s| {
        let q = effective_quality_dim(s.width, s.height);
        q > 0 && q <= target_height
    });

    streams.iter().max_by(|a, b| {
        // 1. Prefer SDR over HDR on modest displays/GPUs
        let sdr_cmp = (!a.hdr).cmp(&(!b.hdr));
        if sdr_cmp != std::cmp::Ordering::Equal {
            return sdr_cmp;
        }

        let aq = effective_quality_dim(a.width, a.height);
        let bq = effective_quality_dim(b.width, b.height);

        if has_within_target {
            let a_ok = aq > 0 && aq <= target_height;
            let b_ok = bq > 0 && bq <= target_height;
            if a_ok != b_ok {
                return a_ok.cmp(&b_ok);
            }
            if aq != bq {
                return aq.cmp(&bq);
            }
        } else if aq != bq {
            // All streams exceed target: pick smallest height first
            return bq.cmp(&aq);
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

fn effective_quality_dim(width: u32, height: u32) -> u32 {
    if width > 0 && height > 0 && width < height {
        width
    } else {
        height
    }
}

fn verified_entry_has_hd(entry: &VerifiedPoolEntry) -> bool {
    entry
        .pool
        .adaptive_video
        .iter()
        .any(|v| v.width.max(v.height) >= 720)
        || entry
            .pool
            .muxed
            .iter()
            .any(|v| v.width.max(v.height) >= 720)
        || entry
            .hls_manifest
            .as_deref()
            .map(|m| {
                m.contains("x720")
                    || m.contains("720x")
                    || m.contains("x1080")
                    || m.contains("1080x")
            })
            .unwrap_or(false)
}

#[derive(Clone, Debug)]
enum CatalogVideoSource {
    Adaptive { url: String },
    Muxed { url: String },
    Hls { uri: String, paired_audio: Option<String> },
}

#[derive(Clone, Debug)]
struct CatalogVideo {
    format_id: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u64,
    filesize: Option<u64>,
    hdr: bool,
    codec: SimpleVideoCodec,
    has_embedded_or_paired_audio: bool,
    source: CatalogVideoSource,
}

impl CatalogVideo {
    fn vcodec_name(&self) -> &'static str {
        match self.codec {
            SimpleVideoCodec::H264 => "avc1",
            SimpleVideoCodec::Vp9 => "vp9",
            SimpleVideoCodec::Mp4v => "mp4v",
            SimpleVideoCodec::Av1 => "av01",
            SimpleVideoCodec::Unknown => "avc1",
        }
    }

    fn acodec_name(&self) -> &'static str {
        if self.has_embedded_or_paired_audio {
            "mp4a.40.2"
        } else {
            "none"
        }
    }

    fn to_ytdl_format_json(&self) -> Value {
        let kbps = ((self.bitrate_bps.max(150_000) as f64) / 100.0).round() / 10.0;
        let mut obj = serde_json::Map::new();
        obj.insert("format_id".to_string(), json!(self.format_id));
        obj.insert("vcodec".to_string(), json!(self.vcodec_name()));
        obj.insert("acodec".to_string(), json!(self.acodec_name()));
        obj.insert("width".to_string(), json!(self.width));
        obj.insert("height".to_string(), json!(self.height));
        obj.insert(
            "resolution".to_string(),
            json!(format!("{}x{}", self.width, self.height)),
        );
        obj.insert("fps".to_string(), json!(self.fps));
        obj.insert("vbr".to_string(), json!(kbps));
        obj.insert("tbr".to_string(), json!(kbps));
        if let Some(sz) = self.filesize {
            obj.insert("filesize".to_string(), json!(sz));
        }
        Value::Object(obj)
    }
}

#[derive(Clone, Debug)]
struct CatalogAudio {
    format_id: String,
    url: String,
    bitrate_bps: u32,
    sample_rate: u32,
    filesize: Option<u64>,
    codec: SimpleAudioCodec,
    is_direct: bool,
}

impl CatalogAudio {
    fn acodec_name(&self) -> &'static str {
        match self.codec {
            SimpleAudioCodec::Aac => "mp4a.40.2",
            SimpleAudioCodec::Opus => "opus",
            SimpleAudioCodec::Other => "mp4a",
        }
    }

    fn to_ytdl_format_json(&self) -> Value {
        let kbps = ((self.bitrate_bps.max(48_000) as f64) / 100.0).round() / 10.0;
        let mut obj = serde_json::Map::new();
        obj.insert("format_id".to_string(), json!(self.format_id));
        obj.insert("vcodec".to_string(), json!("none"));
        obj.insert("acodec".to_string(), json!(self.acodec_name()));
        obj.insert("abr".to_string(), json!(kbps));
        obj.insert("tbr".to_string(), json!(kbps));
        obj.insert("asr".to_string(), json!(self.sample_rate));
        if let Some(sz) = self.filesize {
            obj.insert("filesize".to_string(), json!(sz));
        }
        Value::Object(obj)
    }
}

fn pick_best_catalog_audio(streams: &[CatalogAudio]) -> Option<&CatalogAudio> {
    streams.iter().max_by(|a, b| {
        let codec_rank = |c: SimpleAudioCodec| match c {
            SimpleAudioCodec::Aac => 3u8,
            SimpleAudioCodec::Opus => 2u8,
            SimpleAudioCodec::Other => 1u8,
        };
        let a_good = (96_000..=192_000).contains(&a.bitrate_bps);
        let b_good = (96_000..=192_000).contains(&b.bitrate_bps);
        a.is_direct
            .cmp(&b.is_direct)
            .then_with(|| a_good.cmp(&b_good))
            .then_with(|| codec_rank(a.codec).cmp(&codec_rank(b.codec)))
            .then_with(|| a.bitrate_bps.cmp(&b.bitrate_bps))
    })
}

fn resolve_from_verified_entry(
    entry: &VerifiedPoolEntry,
    default_quality: Quality,
    ytdl_format_override: Option<&str>,
) -> Option<ResolvedPlayback> {
    let pool = &entry.pool;
    let (hls_audio_tracks, hls_variants) = entry
        .hls_manifest
        .as_deref()
        .map(parse_hls_master_entries)
        .unwrap_or_default();

    let mut audio_entries: Vec<CatalogAudio> = Vec::new();
    let mut seen_audio_ids: HashSet<String> = HashSet::new();
    for (idx, a) in pool.audio.iter().enumerate() {
        let base_id = if a.itag > 0 {
            format!("a{}", a.itag)
        } else {
            format!("a-{}", idx + 1)
        };
        let format_id = if seen_audio_ids.insert(base_id.clone()) {
            base_id
        } else {
            let alt = format!("a{}-{}", a.itag, idx + 1);
            seen_audio_ids.insert(alt.clone());
            alt
        };
        audio_entries.push(CatalogAudio {
            format_id,
            url: a.url.clone(),
            bitrate_bps: a.bitrate,
            sample_rate: a.sample_rate,
            filesize: a.filesize,
            codec: a.codec,
            is_direct: true,
        });
    }

    if audio_entries.is_empty() {
        if let Some(aud) = hls_audio_tracks
            .iter()
            .find(|a| a.is_default)
            .or_else(|| hls_audio_tracks.first())
        {
            audio_entries.push(CatalogAudio {
                format_id: "hls-audio".to_string(),
                url: aud.uri.clone(),
                bitrate_bps: 128_000,
                sample_rate: 44_100,
                filesize: None,
                codec: SimpleAudioCodec::Aac,
                is_direct: false,
            });
        }
    }

    let has_direct_audio = audio_entries.iter().any(|a| a.is_direct);
    let mut raw_videos: Vec<CatalogVideo> = Vec::new();

    if has_direct_audio {
        for v in &pool.adaptive_video {
            if v.height == 0 {
                continue;
            }
            let width = if v.width > 0 {
                v.width
            } else {
                (v.height * 16 + 4) / 9
            };
            let format_id = if v.itag > 0 {
                format!("v{}", v.itag)
            } else {
                format!("v-{}", v.height)
            };
            raw_videos.push(CatalogVideo {
                format_id,
                width,
                height: v.height,
                fps: v.fps.max(1),
                bitrate_bps: v.bitrate as u64,
                filesize: v.filesize,
                hdr: v.hdr,
                codec: v.codec,
                has_embedded_or_paired_audio: false,
                source: CatalogVideoSource::Adaptive { url: v.url.clone() },
            });
        }
    }

    for m in &pool.muxed {
        if m.height == 0 {
            continue;
        }
        let width = if m.width > 0 {
            m.width
        } else {
            (m.height * 16 + 4) / 9
        };
        let format_id = if m.itag > 0 {
            format!("m{}", m.itag)
        } else {
            format!("m-{}", m.height)
        };
        raw_videos.push(CatalogVideo {
            format_id,
            width,
            height: m.height,
            fps: m.fps.max(1),
            bitrate_bps: m.bitrate as u64,
            filesize: m.filesize,
            hdr: m.hdr,
            codec: m.codec,
            has_embedded_or_paired_audio: true,
            source: CatalogVideoSource::Muxed { url: m.url.clone() },
        });
    }

    for hv in &hls_variants {
        if hv.height == 0 {
            continue;
        }
        let width = if hv.width > 0 {
            hv.width
        } else {
            (hv.height * 16 + 4) / 9
        };
        let paired_audio = hv.audio_group.as_ref().and_then(|grp| {
            hls_audio_tracks
                .iter()
                .filter(|a| &a.group_id == grp)
                .find(|a| a.is_default)
                .or_else(|| hls_audio_tracks.iter().find(|a| &a.group_id == grp))
                .map(|a| a.uri.clone())
        });
        raw_videos.push(CatalogVideo {
            format_id: format!("hls-{}", hv.height),
            width,
            height: hv.height,
            fps: hv.fps.max(30),
            bitrate_bps: hv.bandwidth,
            filesize: None,
            hdr: false,
            codec: hv.codec,
            has_embedded_or_paired_audio: true,
            source: CatalogVideoSource::Hls {
                uri: hv.uri.clone(),
                paired_audio,
            },
        });
    }

    // Deduplicate to a single best hardware-decoded stream per height (excludes AV1 whenever
    // H.264/VP9 exists at that height, matching Intel UHD 605 hardware decode & quality-menu.lua)
    let mut distinct_heights: Vec<u32> = raw_videos.iter().map(|v| v.height).collect();
    distinct_heights.sort_unstable_by(|a, b| b.cmp(a));
    distinct_heights.dedup();

    let mut video_entries: Vec<CatalogVideo> = Vec::with_capacity(distinct_heights.len());
    let mut used_vid_ids: HashSet<String> = HashSet::new();

    for h in distinct_heights {
        let q_dim_sample = raw_videos
            .iter()
            .find(|v| v.height == h)
            .map(|v| effective_quality_dim(v.width, v.height))
            .unwrap_or(h);

        let source_rank = |src: &CatalogVideoSource| -> u8 {
            if q_dim_sample <= 360 {
                match src {
                    CatalogVideoSource::Muxed { .. } => 3,
                    CatalogVideoSource::Adaptive { .. } => 2,
                    CatalogVideoSource::Hls { .. } => 1,
                }
            } else {
                match src {
                    CatalogVideoSource::Adaptive { .. } => 3,
                    CatalogVideoSource::Muxed { .. } => 2,
                    CatalogVideoSource::Hls { .. } => 1,
                }
            }
        };

        if let Some(mut best) = raw_videos
            .iter()
            .filter(|v| v.height == h)
            .max_by(|a, b| {
                (!a.hdr)
                    .cmp(&(!b.hdr))
                    .then_with(|| {
                        (a.codec != SimpleVideoCodec::Av1).cmp(&(b.codec != SimpleVideoCodec::Av1))
                    })
                    .then_with(|| source_rank(&a.source).cmp(&source_rank(&b.source)))
                    .then_with(|| a.codec.hardware_rank().cmp(&b.codec.hardware_rank()))
                    .then_with(|| (a.fps <= 60).cmp(&(b.fps <= 60)))
                    .then_with(|| a.fps.cmp(&b.fps))
                    .then_with(|| a.bitrate_bps.cmp(&b.bitrate_bps))
            })
            .cloned()
        {
            if !used_vid_ids.insert(best.format_id.clone()) {
                best.format_id = format!("{}-{}p", best.format_id, best.height);
                used_vid_ids.insert(best.format_id.clone());
            }
            video_entries.push(best);
        }
    }

    let best_default_audio = pick_best_catalog_audio(&audio_entries);

    let pick_default_video = |target_h: u32| -> Option<&CatalogVideo> {
        if video_entries.is_empty() {
            return None;
        }
        let has_within = video_entries
            .iter()
            .any(|v| effective_quality_dim(v.width, v.height) <= target_h);
        video_entries.iter().max_by(|a, b| {
            let aq = effective_quality_dim(a.width, a.height);
            let bq = effective_quality_dim(b.width, b.height);
            if has_within {
                let a_ok = aq <= target_h;
                let b_ok = bq <= target_h;
                a_ok.cmp(&b_ok).then_with(|| aq.cmp(&bq))
            } else {
                bq.cmp(&aq)
            }
        })
    };

    let (chosen_video, chosen_audio) = if let Some(ovr) =
        ytdl_format_override.map(str::trim).filter(|s| !s.is_empty())
    {
        let parts: Vec<&str> = ovr.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
        let vid_match = parts
            .iter()
            .find_map(|tok| video_entries.iter().find(|v| v.format_id == *tok));
        let aud_match = parts
            .iter()
            .find_map(|tok| audio_entries.iter().find(|a| a.format_id == *tok));

        if vid_match.is_some() || aud_match.is_some() {
            (vid_match, aud_match)
        } else if default_quality == Quality::AudioOnly {
            (None, best_default_audio)
        } else {
            (
                pick_default_video(default_quality.target_height().unwrap_or(360)),
                best_default_audio,
            )
        }
    } else if default_quality == Quality::AudioOnly {
        (None, best_default_audio)
    } else {
        (
            pick_default_video(default_quality.target_height().unwrap_or(360)),
            best_default_audio,
        )
    };

    let (primary_url, audio_url, is_audio_only) = match (chosen_video, chosen_audio) {
        (Some(vid), aud_opt) => match &vid.source {
            CatalogVideoSource::Adaptive { url } => {
                let direct_aud = aud_opt.filter(|a| a.is_direct).or_else(|| {
                    audio_entries
                        .iter()
                        .filter(|a| a.is_direct)
                        .max_by_key(|a| a.bitrate_bps)
                })?;
                (url.clone(), Some(direct_aud.url.clone()), false)
            }
            CatalogVideoSource::Muxed { url } => (url.clone(), None, false),
            CatalogVideoSource::Hls { uri, paired_audio } => {
                (uri.clone(), paired_audio.clone(), false)
            }
        },
        (None, Some(aud)) => (aud.url.clone(), None, true),
        (None, None) => {
            if let Some(lowest_vid) = video_entries.last() {
                let url = match &lowest_vid.source {
                    CatalogVideoSource::Adaptive { url } => url.clone(),
                    CatalogVideoSource::Muxed { url } => url.clone(),
                    CatalogVideoSource::Hls { uri, .. } => uri.clone(),
                };
                (url, None, default_quality == Quality::AudioOnly)
            } else if let Some(ref master_hls) = pool.hls_url {
                (
                    master_hls.clone(),
                    None,
                    default_quality == Quality::AudioOnly,
                )
            } else {
                return None;
            }
        }
    };

    let mut formats_json: Vec<Value> =
        Vec::with_capacity(audio_entries.len() + video_entries.len());
    for a in &audio_entries {
        formats_json.push(a.to_ytdl_format_json());
    }
    for v in &video_entries {
        formats_json.push(v.to_ytdl_format_json());
    }

    let mut requested_json: Vec<Value> = Vec::with_capacity(2);
    if let Some(v) = chosen_video {
        requested_json.push(v.to_ytdl_format_json());
    }
    if let Some(a) = chosen_audio {
        requested_json.push(a.to_ytdl_format_json());
    }

    let resolved_title = pool.title.clone().unwrap_or_default();
    let ytdl_json = serde_json::to_string(&json!({
        "title": resolved_title,
        "formats": formats_json,
        "requested_formats": requested_json,
    }))
    .ok();

    Some(ResolvedPlayback {
        primary_url,
        audio_url,
        user_agent: pool.user_agent.clone(),
        referer: pool.referer.clone(),
        is_audio_only,
        duration_secs: pool.duration_secs,
        title: resolved_title,
        ytdl_json,
    })
}

#[cfg(test)]
fn select_direct_playback(pool: &CandidatePool, quality: Quality) -> Option<ResolvedPlayback> {
    let entry = VerifiedPoolEntry {
        video_id: "test".to_string(),
        fetched_at: Instant::now(),
        pool: pool.clone(),
        hls_manifest: None,
    };
    resolve_from_verified_entry(&entry, quality, None)
}

#[derive(Clone, Debug)]
struct HlsAudioTrack {
    group_id: String,
    uri: String,
    is_default: bool,
}

#[derive(Clone, Debug)]
struct HlsVariant {
    uri: String,
    width: u32,
    height: u32,
    fps: u32,
    bandwidth: u64,
    codec: SimpleVideoCodec,
    audio_group: Option<String>,
}

fn extract_m3u8_attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while !rest.is_empty() {
        rest = rest.trim_start_matches([',', ' ', '\t']);
        if rest.is_empty() {
            break;
        }
        let Some((k, after_eq)) = rest.split_once('=') else {
            break;
        };
        let k = k.trim();
        let (val, tail) = if let Some(quoted) = after_eq.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((v, rem)) => (v, rem),
                None => (quoted, ""),
            }
        } else {
            match after_eq.split_once(',') {
                Some((v, rem)) => (v.trim(), rem),
                None => (after_eq.trim(), ""),
            }
        };
        if k.eq_ignore_ascii_case(key) {
            return Some(val);
        }
        rest = tail;
    }
    None
}

fn parse_hls_master_entries(manifest: &str) -> (Vec<HlsAudioTrack>, Vec<HlsVariant>) {
    let mut audio_tracks: Vec<HlsAudioTrack> = Vec::new();
    let mut variants: Vec<HlsVariant> = Vec::new();
    let mut pending_stream_inf: Option<&str> = None;

    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-MEDIA:") {
            if extract_m3u8_attr(attrs, "TYPE")
                .map(|t| t.eq_ignore_ascii_case("AUDIO"))
                .unwrap_or(false)
            {
                if let (Some(group_id), Some(uri)) = (
                    extract_m3u8_attr(attrs, "GROUP-ID"),
                    extract_m3u8_attr(attrs, "URI"),
                ) {
                    if !uri.is_empty() {
                        let is_default = extract_m3u8_attr(attrs, "DEFAULT")
                            .map(|d| d.eq_ignore_ascii_case("YES"))
                            .unwrap_or(false);
                        audio_tracks.push(HlsAudioTrack {
                            group_id: group_id.to_string(),
                            uri: uri.to_string(),
                            is_default,
                        });
                    }
                }
            }
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_stream_inf = Some(attrs);
            continue;
        }
        if !line.starts_with('#') {
            if let Some(attrs) = pending_stream_inf.take() {
                let (width, height) = extract_m3u8_attr(attrs, "RESOLUTION")
                    .and_then(|res| res.split_once('x'))
                    .map(|(w, h)| {
                        (
                            w.parse::<u32>().unwrap_or(0),
                            h.parse::<u32>().unwrap_or(0),
                        )
                    })
                    .unwrap_or((0, 0));
                let fps = extract_m3u8_attr(attrs, "FRAME-RATE")
                    .and_then(|f| f.parse::<f64>().ok())
                    .map(|f| f.round() as u32)
                    .filter(|&f| f > 0)
                    .unwrap_or(30);
                let bandwidth = extract_m3u8_attr(attrs, "AVERAGE-BANDWIDTH")
                    .or_else(|| extract_m3u8_attr(attrs, "BANDWIDTH"))
                    .and_then(|b| b.parse::<u64>().ok())
                    .unwrap_or(0);
                let codecs = extract_m3u8_attr(attrs, "CODECS").unwrap_or("");
                let codec = if codecs.contains("avc1") {
                    SimpleVideoCodec::H264
                } else if codecs.contains("vp09") || codecs.contains("vp9") {
                    SimpleVideoCodec::Vp9
                } else if codecs.contains("av01") {
                    SimpleVideoCodec::Av1
                } else {
                    SimpleVideoCodec::Unknown
                };
                let audio_group = extract_m3u8_attr(attrs, "AUDIO").map(|s| s.to_string());
                variants.push(HlsVariant {
                    uri: line.to_string(),
                    width,
                    height,
                    fps,
                    bandwidth,
                    codec,
                    audio_group,
                });
            }
        }
    }

    (audio_tracks, variants)
}

fn parse_hls_master_playlist(
    manifest: &str,
    quality: Quality,
) -> Option<(String, Option<String>)> {
    let (audio_tracks, variants) = parse_hls_master_entries(manifest);

    if quality == Quality::AudioOnly {
        if let Some(aud) = audio_tracks
            .iter()
            .find(|a| a.is_default)
            .or_else(|| audio_tracks.first())
        {
            return Some((aud.uri.clone(), None));
        }
        if let Some(lowest) = variants.iter().min_by_key(|v| (v.height, v.bandwidth)) {
            return Some((lowest.uri.clone(), None));
        }
        return None;
    }

    if variants.is_empty() {
        return None;
    }

    let target_h = quality.target_height().unwrap_or(360);
    let has_within_target = variants
        .iter()
        .any(|v| effective_quality_dim(v.width, v.height) > 0 && effective_quality_dim(v.width, v.height) <= target_h);

    let chosen = variants.iter().max_by(|a, b| {
        let aq = effective_quality_dim(a.width, a.height);
        let bq = effective_quality_dim(b.width, b.height);
        if has_within_target {
            let a_ok = aq > 0 && aq <= target_h;
            let b_ok = bq > 0 && bq <= target_h;
            if a_ok != b_ok {
                return a_ok.cmp(&b_ok);
            }
            if aq != bq {
                return aq.cmp(&bq);
            }
        } else if aq != bq {
            return bq.cmp(&aq);
        }

        a.codec
            .hardware_rank()
            .cmp(&b.codec.hardware_rank())
            .then_with(|| a.bandwidth.cmp(&b.bandwidth))
    })?;

    let paired_audio = chosen.audio_group.as_ref().and_then(|grp| {
        audio_tracks
            .iter()
            .filter(|a| &a.group_id == grp)
            .find(|a| a.is_default)
            .or_else(|| audio_tracks.iter().find(|a| &a.group_id == grp))
            .map(|a| a.uri.clone())
    });

    Some((chosen.uri.clone(), paired_audio))
}

struct ChannelFallback<'a> {
    id: &'a str,
    name: &'a str,
    avatar_url: &'a str,
}

fn normalize_thumb_url(u: &str) -> String {
    if let Some(stripped) = u.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        u.to_string()
    }
}

fn pick_thumbnail_url(thumbs: &[Thumbnail], min_width: u32) -> String {
    if thumbs.is_empty() {
        return String::new();
    }
    if let Some(found) = thumbs
        .iter()
        .filter(|t| t.width >= min_width)
        .min_by_key(|t| t.width)
    {
        return normalize_thumb_url(&found.url);
    }
    thumbs
        .iter()
        .max_by_key(|t| t.width)
        .or_else(|| thumbs.last())
        .map(|t| normalize_thumb_url(&t.url))
        .unwrap_or_default()
}

fn pick_json_sources_url(sources: Option<&Vec<Value>>, min_width: u64) -> Option<String> {
    let arr = sources?;
    if arr.is_empty() {
        return None;
    }
    let best = arr
        .iter()
        .filter_map(|s| {
            let url = s.get("url").and_then(|v| v.as_str())?;
            let w = s.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
            Some((w, url))
        })
        .filter(|(w, _)| *w >= min_width)
        .min_by_key(|(w, _)| *w)
        .or_else(|| {
            arr.iter()
                .filter_map(|s| {
                    let url = s.get("url").and_then(|v| v.as_str())?;
                    let w = s.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                    Some((w, url))
                })
                .max_by_key(|(w, _)| *w)
        })?;
    Some(normalize_thumb_url(best.1))
}

fn parse_compact_count_str(text: &str) -> Option<u64> {
    let token = text
        .split_whitespace()
        .next()?
        .trim()
        .replace(',', "");
    if token.is_empty() {
        return None;
    }
    let (num_part, mult) = match token.chars().last()? {
        'K' | 'k' => (&token[..token.len() - 1], 1_000.0),
        'M' | 'm' => (&token[..token.len() - 1], 1_000_000.0),
        'B' | 'b' => (&token[..token.len() - 1], 1_000_000_000.0),
        _ => (token.as_str(), 1.0),
    };
    let val: f64 = num_part.parse().ok()?;
    if val < 0.0 {
        return None;
    }
    Some((val * mult) as u64)
}

fn extract_browse_header_stats(val: &Value) -> ChannelHeaderStats {
    let mut out = ChannelHeaderStats::default();

    if let Some(ph) = val.pointer("/header/pageHeaderRenderer") {
        out.channel_name = ph
            .pointer("/content/pageHeaderViewModel/title/dynamicTextViewModel/text/content")
            .or_else(|| ph.get("pageTitle"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        out.avatar_url = pick_json_sources_url(
            ph.pointer(
                "/content/pageHeaderViewModel/image/decoratedAvatarViewModel/avatar/avatarViewModel/image/sources",
            )
            .and_then(|v| v.as_array()),
            88,
        );

        if let Some(rows) = ph
            .pointer("/content/pageHeaderViewModel/metadata/contentMetadataViewModel/metadataRows")
            .and_then(|v| v.as_array())
        {
            for row in rows {
                if let Some(parts) = row.get("metadataParts").and_then(|v| v.as_array()) {
                    for part in parts {
                        let Some(txt) = part
                            .pointer("/text/content")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        else {
                            continue;
                        };
                        let lower = txt.to_ascii_lowercase();
                        if txt.starts_with('@') && out.handle.is_none() {
                            out.handle = Some(txt.to_string());
                        } else if lower.contains("subscriber") && out.subscriber_count_text.is_none()
                        {
                            out.subscriber_count_text = Some(txt.to_string());
                        } else if lower.contains("video") && out.video_count_text.is_none() {
                            out.video_count_text = Some(txt.to_string());
                        }
                    }
                }
            }
        }
    } else if let Some(c4) = val.pointer("/header/c4TabbedHeaderRenderer") {
        out.channel_name = c4
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        out.handle = c4
            .pointer("/channelHandleText/simpleText")
            .or_else(|| c4.pointer("/channelHandleText/runs/0/text"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.subscriber_count_text = c4
            .pointer("/subscriberCountText/simpleText")
            .or_else(|| c4.pointer("/subscriberCountText/runs/0/text"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.video_count_text = c4
            .pointer("/videosCountText/simpleText")
            .or_else(|| c4.pointer("/videosCountText/runs/0/text"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.avatar_url = pick_json_sources_url(
            c4.pointer("/avatar/thumbnails").and_then(|v| v.as_array()),
            88,
        );
    }

    out
}

fn extract_continuation_token_from_renderer(cont_renderer: &Value) -> Option<String> {
    if let Some(tok) = cont_renderer
        .pointer("/continuationEndpoint/continuationCommand/token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(tok.to_string());
    }
    if let Some(cmds) = cont_renderer
        .pointer("/continuationEndpoint/commandExecutorCommand/commands")
        .and_then(|v| v.as_array())
    {
        for cmd in cmds {
            if let Some(tok) = cmd
                .pointer("/continuationCommand/token")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(tok.to_string());
            }
        }
    }
    None
}

fn parse_short_lockup_item(
    lockup: &Value,
    channel_id: &str,
    fallback: Option<&ChannelFallback<'_>>,
) -> Option<VideoSummary> {
    let vid = lockup
        .pointer("/onTap/innertubeCommand/reelWatchEndpoint/videoId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            lockup
                .pointer("/inlinePlayerData/onVisible/innertubeCommand/watchEndpoint/videoId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            lockup
                .get("entityId")
                .and_then(|v| v.as_str())
                .and_then(|s| s.strip_prefix("shorts-shelf-item-"))
                .filter(|s| !s.is_empty())
        })?;

    let title = lockup
        .pointer("/overlayMetadata/primaryText/content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            lockup
                .get("accessibilityText")
                .and_then(|v| v.as_str())
                .and_then(|s| s.split(" - play Short").next())
                .or_else(|| lockup.get("accessibilityText").and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("Short")
        .to_string();

    let view_count_text = lockup
        .pointer("/overlayMetadata/secondaryText/content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let view_count = parse_compact_count_str(&view_count_text);

    let thumbnail_url = pick_json_sources_url(
        lockup
            .pointer("/thumbnailViewModel/thumbnailViewModel/image/sources")
            .and_then(|v| v.as_array()),
        216,
    )
    .or_else(|| {
        pick_json_sources_url(
            lockup
                .pointer("/thumbnailViewModel/image/sources")
                .and_then(|v| v.as_array()),
            216,
        )
    })
    .or_else(|| {
        pick_json_sources_url(
            lockup.pointer("/thumbnail/sources").and_then(|v| v.as_array()),
            216,
        )
    })
    .or_else(|| {
        pick_json_sources_url(
            lockup
                .pointer("/thumbnail/thumbnails")
                .and_then(|v| v.as_array()),
            216,
        )
    })
    .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg"));

    let published_text = find_published_label(lockup).unwrap_or_default();
    let published_unix = inferred_publish_timestamp(&published_text);
    Some(VideoSummary {
        id: vid.to_string(),
        title,
        channel_id: fallback
            .map(|f| f.id)
            .filter(|s| !s.is_empty())
            .unwrap_or(channel_id)
            .to_string(),
        channel_name: fallback.map(|f| f.name).unwrap_or("").to_string(),
        channel_avatar_url: fallback.map(|f| f.avatar_url).unwrap_or("").to_string(),
        duration_seconds: None,
        duration_text: "SHORTS".to_string(),
        view_count_text,
        view_count,
        published_text,
        published_unix,
        is_short: true,
        thumbnail_url,
        description_snippet: String::new(),
            collaborators: Vec::new(),
    })
}

fn parse_reel_item(
    reel: &Value,
    channel_id: &str,
    fallback: Option<&ChannelFallback<'_>>,
) -> Option<VideoSummary> {
    let vid = reel
        .get("videoId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let title = reel
        .pointer("/headline/simpleText")
        .or_else(|| reel.pointer("/headline/runs/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or("Short")
        .to_string();
    let view_count_text = reel
        .pointer("/viewCountText/simpleText")
        .or_else(|| reel.pointer("/viewCountText/runs/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let view_count = parse_compact_count_str(&view_count_text);
    let thumbnail_url = pick_json_sources_url(
        reel.pointer("/thumbnail/thumbnails")
            .and_then(|v| v.as_array()),
        216,
    )
    .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg"));

    let published_text = find_published_label(reel).unwrap_or_default();
    let published_unix = inferred_publish_timestamp(&published_text);
    Some(VideoSummary {
        id: vid.to_string(),
        title,
        channel_id: fallback
            .map(|f| f.id)
            .filter(|s| !s.is_empty())
            .unwrap_or(channel_id)
            .to_string(),
        channel_name: fallback.map(|f| f.name).unwrap_or("").to_string(),
        channel_avatar_url: fallback.map(|f| f.avatar_url).unwrap_or("").to_string(),
        duration_seconds: None,
        duration_text: "SHORTS".to_string(),
        view_count_text,
        view_count,
        published_text,
        published_unix,
        is_short: true,
        thumbnail_url,
        description_snippet: String::new(),
            collaborators: Vec::new(),
    })
}

fn collect_short_items_from_array(
    arr: &[Value],
    channel_id: &str,
    fallback: Option<&ChannelFallback<'_>>,
    out_items: &mut Vec<VideoSummary>,
    seen_ids: &mut HashSet<String>,
    out_ctoken: &mut Option<String>,
) {
    for entry in arr {
        if let Some(content) = entry.pointer("/richItemRenderer/content") {
            if let Some(lockup) = content.get("shortsLockupViewModel") {
                if let Some(v) = parse_short_lockup_item(lockup, channel_id, fallback) {
                    if seen_ids.insert(v.id.clone()) {
                        out_items.push(v);
                    }
                }
            } else if let Some(reel) = content.get("reelItemRenderer") {
                if let Some(v) = parse_reel_item(reel, channel_id, fallback) {
                    if seen_ids.insert(v.id.clone()) {
                        out_items.push(v);
                    }
                }
            }
        } else if let Some(lockup) = entry.get("shortsLockupViewModel") {
            if let Some(v) = parse_short_lockup_item(lockup, channel_id, fallback) {
                if seen_ids.insert(v.id.clone()) {
                    out_items.push(v);
                }
            }
        } else if let Some(reel) = entry.get("reelItemRenderer") {
            if let Some(v) = parse_reel_item(reel, channel_id, fallback) {
                if seen_ids.insert(v.id.clone()) {
                    out_items.push(v);
                }
            }
        } else if let Some(cont) = entry.get("continuationItemRenderer") {
            if let Some(tok) = extract_continuation_token_from_renderer(cont) {
                *out_ctoken = Some(tok);
            }
        } else if let Some(sub) = entry
            .pointer("/itemSectionRenderer/contents")
            .and_then(|v| v.as_array())
        {
            collect_short_items_from_array(
                sub,
                channel_id,
                fallback,
                out_items,
                seen_ids,
                out_ctoken,
            );
        }
    }
}

fn select_shorts_tab_renderer(val: &Value) -> Option<&Value> {
    let tabs = val
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs")
        .and_then(|v| v.as_array())?;
    tabs.iter()
        .filter_map(|t| t.get("tabRenderer"))
        .find(|tr| {
            tr.pointer("/endpoint/commandMetadata/webCommandMetadata/url")
                .and_then(|v| v.as_str())
                .map(|u| u.ends_with("/shorts"))
                .unwrap_or(false)
        })
        .or_else(|| {
            tabs.iter()
                .filter_map(|t| t.get("tabRenderer"))
                .find(|tr| tr.get("selected").and_then(|v| v.as_bool()) == Some(true))
        })
}

fn parse_shorts_browse_response(
    val: &Value,
    channel_id: &str,
    fallback: Option<&ChannelFallback<'_>>,
) -> PagedVideos {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut ctoken = None;

    if let Some(tab) = select_shorts_tab_renderer(val) {
        if let Some(arr) = tab
            .pointer("/content/richGridRenderer/contents")
            .and_then(|v| v.as_array())
        {
            collect_short_items_from_array(arr, channel_id, fallback, &mut items, &mut seen, &mut ctoken);
        } else if let Some(arr) = tab
            .pointer("/content/sectionListRenderer/contents")
            .and_then(|v| v.as_array())
        {
            collect_short_items_from_array(arr, channel_id, fallback, &mut items, &mut seen, &mut ctoken);
        }
    }

    if let Some(actions) = val
        .get("onResponseReceivedActions")
        .and_then(|v| v.as_array())
    {
        for action in actions {
            if let Some(arr) = action
                .pointer("/appendContinuationItemsAction/continuationItems")
                .and_then(|v| v.as_array())
            {
                collect_short_items_from_array(
                    arr,
                    channel_id,
                    fallback,
                    &mut items,
                    &mut seen,
                    &mut ctoken,
                );
            }
            if let Some(arr) = action
                .pointer("/reloadContinuationItemsCommand/continuationItems")
                .and_then(|v| v.as_array())
            {
                collect_short_items_from_array(
                    arr,
                    channel_id,
                    fallback,
                    &mut items,
                    &mut seen,
                    &mut ctoken,
                );
            }
        }
    }

    let next = ctoken.map(|tok| PageCursor {
        ctoken: tok,
        endpoint: ContinuationEndpoint::Browse,
        visitor_data: None,
        kind: CursorKind::DirectChannelShorts,
    });

    PagedVideos { items, next }
}

fn inferred_publish_timestamp(text: &str) -> Option<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    relative_published_timestamp(text, now)
}

fn find_nested_continuation_token(node: &Value) -> Option<String> {
    match node {
        Value::Array(items) => items.iter().find_map(find_nested_continuation_token),
        Value::Object(map) => {
            if let Some(token) = node
                .pointer("/continuationCommand/token")
                .or_else(|| node.pointer("/continuationEndpoint/continuationCommand/token"))
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
            {
                return Some(token.to_string());
            }
            map.values().find_map(find_nested_continuation_token)
        }
        _ => None,
    }
}

fn extract_channel_sort_chip_token(node: &Value, wanted_label: &str) -> Option<String> {
    match node {
        Value::Array(items) => items
            .iter()
            .find_map(|item| extract_channel_sort_chip_token(item, wanted_label)),
        Value::Object(map) => {
            let label = map
                .get("text")
                .or_else(|| map.get("title"))
                .and_then(|value| extract_yt_text(Some(value)));
            if label
                .as_deref()
                .is_some_and(|label| label.trim().eq_ignore_ascii_case(wanted_label))
            {
                if let Some(token) = find_nested_continuation_token(node) {
                    return Some(token);
                }
            }
            map.values()
                .find_map(|value| extract_channel_sort_chip_token(value, wanted_label))
        }
        _ => None,
    }
}

fn find_published_label(node: &Value) -> Option<String> {
    match node {
        Value::Array(items) => items.iter().find_map(find_published_label),
        Value::Object(map) => {
            const DATE_KEYS: &[&str] = &[
                "publishedTimeText",
                "publishedText",
                "publishDateText",
                "videoPublishedAt",
                "timestampText",
            ];
            for key in DATE_KEYS {
                if let Some(text) = map.get(*key).and_then(|value| extract_yt_text(Some(value))) {
                    return Some(text);
                }
            }
            map.values().find_map(find_published_label)
        }
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            [" ago", "streamed", "premiered", "yesterday", "today"]
                .iter()
                .any(|needle| lower.contains(needle))
                .then(|| text.trim().to_string())
        }
        _ => None,
    }
}

fn extract_yt_text(node: Option<&Value>) -> Option<String> {
    let v = node?;
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(s) = v.get("simpleText").and_then(|x| x.as_str()) {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(s) = v.get("content").and_then(|x| x.as_str()) {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(runs) = v.get("runs").and_then(|x| x.as_array()) {
        let joined: String = runs
            .iter()
            .filter_map(|r| r.get("text").and_then(|x| x.as_str()))
            .collect();
        let t = joined.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    None
}

fn parse_mmss_to_seconds(txt: &str) -> Option<u64> {
    let parts: Vec<&str> = txt.trim().split(':').collect();
    match parts.len() {
        2 => {
            let m: u64 = parts[0].trim().parse().ok()?;
            let s: u64 = parts[1].trim().parse().ok()?;
            Some(m * 60 + s)
        }
        3 => {
            let h: u64 = parts[0].trim().parse().ok()?;
            let m: u64 = parts[1].trim().parse().ok()?;
            let s: u64 = parts[2].trim().parse().ok()?;
            Some(h * 3600 + m * 60 + s)
        }
        _ => None,
    }
}

fn parse_playlist_video_renderer(pvr: &Value) -> Option<VideoSummary> {
    if pvr.get("isPlayable").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    let vid = pvr
        .get("videoId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let title = extract_yt_text(pvr.get("title")).unwrap_or_else(|| "Untitled video".to_string());

    let channel_name = extract_yt_text(pvr.get("shortBylineText")).unwrap_or_default();
    let channel_id = pvr
        .pointer("/shortBylineText/runs/0/navigationEndpoint/browseEndpoint/browseId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let duration_seconds = pvr
        .get("lengthSeconds")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .or_else(|| {
            extract_yt_text(pvr.get("lengthText")).and_then(|s| parse_mmss_to_seconds(&s))
        });
    let duration_text = extract_yt_text(pvr.get("lengthText"))
        .or_else(|| duration_seconds.map(format_duration))
        .unwrap_or_default();

    let mut view_count_text = String::new();
    let mut published_text = String::new();
    if let Some(runs) = pvr.pointer("/videoInfo/runs").and_then(|v| v.as_array()) {
        for r in runs {
            if let Some(t) = r.get("text").and_then(|v| v.as_str()).map(str::trim) {
                if t.is_empty() || t == "•" || t == "·" {
                    continue;
                }
                let lower = t.to_ascii_lowercase();
                if lower.contains("view") || lower.contains("watching") {
                    view_count_text = t.to_string();
                } else if lower.contains("ago") || lower.contains("streamed") {
                    published_text = t.to_string();
                }
            }
        }
    }
    let view_count = parse_compact_count_str(&view_count_text);

    let thumbnail_url = pick_json_sources_url(
        pvr.pointer("/thumbnail/thumbnails")
            .and_then(|v| v.as_array()),
        320,
    )
    .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{vid}/mqdefault.jpg"));

    let is_short = pvr
        .pointer("/navigationEndpoint/reelWatchEndpoint")
        .is_some()
        || duration_text.eq_ignore_ascii_case("SHORTS");

    Some(VideoSummary {
        id: vid.to_string(),
        title,
        channel_id,
        channel_name,
        channel_avatar_url: String::new(),
        duration_seconds,
        duration_text,
        view_count_text,
        view_count,
        published_unix: inferred_publish_timestamp(&published_text),
        published_text,
        is_short,
        thumbnail_url,
        description_snippet: String::new(),
            collaborators: Vec::new(),
    })
}

fn parse_video_renderer_json(vr: &Value) -> Option<VideoSummary> {
    let vid = vr
        .get("videoId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let title = extract_yt_text(vr.get("title")).unwrap_or_else(|| "Untitled video".to_string());
    let byline = vr
        .get("ownerText")
        .or_else(|| vr.get("longBylineText"))
        .or_else(|| vr.get("shortBylineText"));
    let mut channel_name = extract_yt_text(byline).unwrap_or_default();
    let mut channel_id = byline
        .and_then(|b| b.pointer("/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mut channel_avatar_url = pick_json_sources_url(
        vr.pointer("/channelThumbnailSupportedRenderers/channelThumbnailWithLinkRenderer/thumbnail/thumbnails")
            .and_then(|v| v.as_array()),
        64,
    )
    .unwrap_or_default();

    let collaborators = parse_video_collaborators(vr);
    if let Some(primary) = collaborators.first() {
        if !primary.id.is_empty() {
            channel_id = primary.id.clone();
        }
        if !primary.name.is_empty() {
            channel_name = primary.name.clone();
        }
        if !primary.avatar_url.is_empty() {
            channel_avatar_url = primary.avatar_url.clone();
        }
    }

    let mut duration_text = extract_yt_text(vr.get("lengthText")).unwrap_or_default();
    let mut is_short = vr
        .pointer("/navigationEndpoint/reelWatchEndpoint")
        .is_some();
    if let Some(overlays) = vr.get("thumbnailOverlays").and_then(|v| v.as_array()) {
        for ov in overlays {
            if let Some(tr) = ov.get("thumbnailOverlayTimeStatusRenderer") {
                if duration_text.is_empty() {
                    if let Some(t) = extract_yt_text(tr.get("text")) {
                        duration_text = t;
                    }
                }
                if tr
                    .get("style")
                    .and_then(|v| v.as_str())
                    .map(|s| s.eq_ignore_ascii_case("SHORTS"))
                    .unwrap_or(false)
                {
                    is_short = true;
                }
            }
        }
    }
    if duration_text.eq_ignore_ascii_case("SHORTS") {
        is_short = true;
    }
    let duration_seconds = parse_mmss_to_seconds(&duration_text);
    let view_count_text = extract_yt_text(vr.get("shortViewCountText"))
        .or_else(|| extract_yt_text(vr.get("viewCountText")))
        .unwrap_or_default();
    let view_count = parse_compact_count_str(&view_count_text);
    let published_text = extract_yt_text(vr.get("publishedTimeText")).unwrap_or_default();
    let description_snippet = vr
        .pointer("/detailedMetadataSnippets/0/snippetText")
        .and_then(|v| extract_yt_text(Some(v)))
        .or_else(|| extract_yt_text(vr.get("descriptionSnippet")))
        .unwrap_or_default();

    let thumbnail_url = if is_short {
        pick_json_sources_url(
            vr.pointer("/navigationEndpoint/reelWatchEndpoint/thumbnail/thumbnails")
                .and_then(|v| v.as_array()),
            216,
        )
        .or_else(|| {
            pick_json_sources_url(
                vr.pointer("/thumbnail/thumbnails")
                    .and_then(|v| v.as_array()),
                216,
            )
        })
        .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg"))
    } else {
        pick_json_sources_url(
            vr.pointer("/thumbnail/thumbnails")
                .and_then(|v| v.as_array()),
            320,
        )
        .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{vid}/mqdefault.jpg"))
    };

    Some(VideoSummary {
        id: vid.to_string(),
        title,
        channel_id,
        channel_name,
        channel_avatar_url,
        duration_seconds,
        duration_text,
        view_count_text,
        view_count,
        published_unix: inferred_publish_timestamp(&published_text),
        published_text,
        is_short,
        thumbnail_url,
        description_snippet,
        collaborators,
    })
}

fn parse_channel_renderer_json(cr: &Value) -> Option<ChannelSearchSummary> {
    let cid = cr
        .get("channelId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let name = extract_yt_text(cr.get("title")).unwrap_or_else(|| "Channel".to_string());
    let sub_txt = extract_yt_text(cr.get("videoCountText")).unwrap_or_default();
    let real_sub_txt = extract_yt_text(cr.get("subscriberCountText")).unwrap_or_default();

    let (handle, subscriber_count_text) = if sub_txt.starts_with('@') {
        (sub_txt, real_sub_txt)
    } else if real_sub_txt.starts_with('@') {
        (real_sub_txt, sub_txt)
    } else {
        let h = cr
            .pointer("/navigationEndpoint/browseEndpoint/canonicalBaseUrl")
            .and_then(|v| v.as_str())
            .and_then(|u| u.strip_prefix('/'))
            .filter(|s| s.starts_with('@'))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("@{}", name.to_lowercase().replace(' ', "")));
        let subs = if !real_sub_txt.is_empty() {
            real_sub_txt
        } else {
            sub_txt
        };
        (h, subs)
    };

    let description = extract_yt_text(cr.get("descriptionSnippet")).unwrap_or_default();
    let avatar_url = pick_json_sources_url(
        cr.pointer("/thumbnail/thumbnails")
            .and_then(|v| v.as_array()),
        64,
    )
    .unwrap_or_default();

    Some(ChannelSearchSummary {
        id: cid.to_string(),
        name,
        handle,
        subscriber_count_text,
        description,
        avatar_url,
    })
}

fn parse_playlist_renderer_json(
    pr: &Value,
    fallback_channel_name: &str,
) -> Option<PlaylistSearchSummary> {
    let pid = pr
        .get("playlistId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let name = extract_yt_text(pr.get("title")).unwrap_or_else(|| "Playlist".to_string());
    let channel_name = extract_yt_text(pr.get("shortBylineText"))
        .or_else(|| extract_yt_text(pr.get("longBylineText")))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_channel_name.to_string());

    let video_count_text = extract_yt_text(pr.get("videoCountText"))
        .or_else(|| extract_yt_text(pr.get("videoCountShortText")))
        .map(|s| {
            if s.to_ascii_lowercase().contains("video") {
                s
            } else {
                format!("{s} videos")
            }
        })
        .or_else(|| {
            pr.get("videoCount")
                .and_then(|v| v.as_str().or_else(|| None))
                .map(|s| format!("{s} videos"))
        })
        .unwrap_or_else(|| "Playlist".to_string());

    let thumbnail_url = pick_json_sources_url(
        pr.pointer("/thumbnails/0/thumbnails")
            .or_else(|| pr.pointer("/thumbnail/thumbnails"))
            .and_then(|v| v.as_array()),
        320,
    )
    .unwrap_or_default();

    Some(PlaylistSearchSummary {
        id: pid.to_string(),
        name,
        channel_name,
        video_count_text,
        thumbnail_url,
    })
}

fn parse_playlist_lockup_json(
    lockup: &Value,
    fallback_channel_name: &str,
) -> Option<PlaylistSearchSummary> {
    let content_type = lockup
        .get("contentType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cid = lockup
        .get("contentId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;

    let is_playlist_type = content_type == "LOCKUP_CONTENT_TYPE_PLAYLIST"
        || content_type == "LOCKUP_CONTENT_TYPE_PODCAST"
        || content_type == "LOCKUP_CONTENT_TYPE_ALBUM"
        || (content_type.is_empty()
            && (cid.starts_with("PL")
                || cid.starts_with("OL")
                || cid.starts_with("UU")
                || cid.starts_with("FL")
                || cid.starts_with("RD")));
    if !is_playlist_type {
        return None;
    }
    // Skip infinite auto-generated Radio Mixes ("RD...") that cannot be browsed as normal playlists
    if cid.starts_with("RD") {
        return None;
    }

    let name = lockup
        .pointer("/metadata/lockupMetadataViewModel/title/content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Playlist")
        .to_string();

    let mut channel_name = String::new();
    if let Some(rows) = lockup
        .pointer("/metadata/lockupMetadataViewModel/metadata/contentMetadataViewModel/metadataRows")
        .and_then(|v| v.as_array())
    {
        for row in rows {
            if let Some(parts) = row.get("metadataParts").and_then(|v| v.as_array()) {
                for part in parts {
                    if let Some(txt) = part
                        .pointer("/text/content")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        let lower = txt.to_ascii_lowercase();
                        if lower != "playlist"
                            && lower != "podcast"
                            && lower != "course"
                            && !lower.starts_with("updated ")
                            && !lower.contains("full playlist")
                            && !lower.contains("view full")
                        {
                            channel_name = txt.strip_prefix("by ").unwrap_or(txt).trim().to_string();
                            break;
                        }
                    }
                }
            }
            if !channel_name.is_empty() {
                break;
            }
        }
    }
    if channel_name.is_empty() && !fallback_channel_name.is_empty() {
        channel_name = fallback_channel_name.to_string();
    }

    let thumb_vm = lockup
        .pointer("/contentImage/collectionThumbnailViewModel/primaryThumbnail/thumbnailViewModel")
        .or_else(|| lockup.pointer("/contentImage/thumbnailViewModel"));

    let thumbnail_url = thumb_vm
        .and_then(|tvm| {
            pick_json_sources_url(tvm.pointer("/image/sources").and_then(|v| v.as_array()), 320)
        })
        .unwrap_or_default();

    let mut video_count_text = String::new();
    if let Some(overlays) = thumb_vm
        .and_then(|tvm| tvm.get("overlays"))
        .and_then(|v| v.as_array())
    {
        for ov in overlays {
            if let Some(badges) = ov
                .pointer("/thumbnailOverlayBadgeViewModel/thumbnailBadges")
                .and_then(|v| v.as_array())
            {
                for b in badges {
                    if let Some(t) = b
                        .pointer("/thumbnailBadgeViewModel/text")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        video_count_text = t.to_string();
                        break;
                    }
                }
            }
            if !video_count_text.is_empty() {
                break;
            }
        }
    }
    if video_count_text.is_empty() {
        video_count_text = "Playlist".to_string();
    }

    Some(PlaylistSearchSummary {
        id: cid.to_string(),
        name,
        channel_name,
        video_count_text,
        thumbnail_url,
    })
}

fn push_proto_varint(out: &mut Vec<u8>, mut val: u64) {
    while val >= 0x80 {
        out.push(((val as u8) & 0x7F) | 0x80);
        val >>= 7;
    }
    out.push(val as u8);
}

fn push_proto_bytes_field(out: &mut Vec<u8>, field_number: u64, data: &[u8]) {
    let tag = (field_number << 3) | 2;
    push_proto_varint(out, tag);
    push_proto_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn encode_base64_url_safe_pct(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4) / 3 + 8);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push_str("%3D%3D");
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push_str("%3D");
    }
    out
}

/// Builds YouTube's `PlaylistContinuation` protobuf token (`field 80226972`) used by YouTube's
/// new `lockupViewModel` / Podcast / Course playlist pages to fetch initial items.
fn build_initial_playlist_continuation_token(playlist_id: &str) -> String {
    let browse_id = format!("VL{playlist_id}");
    let mut inner = Vec::with_capacity(browse_id.len() + playlist_id.len() + 32);
    push_proto_bytes_field(&mut inner, 2, browse_id.as_bytes());
    push_proto_bytes_field(&mut inner, 3, b"CADCBgIIAA%3D%3D");
    push_proto_bytes_field(&mut inner, 35, playlist_id.as_bytes());

    let mut outer = Vec::with_capacity(inner.len() + 12);
    push_proto_bytes_field(&mut outer, 80_226_972, &inner);
    encode_base64_url_safe_pct(&outer)
}

fn shuffle_videos_seeded(items: &mut [VideoSummary], mut state: u64) {
    if state == 0 {
        state = 0xCAFE_BABE_DEAD_BEEF;
    }
    for i in (1..items.len()).rev() {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let j = (z as usize) % (i + 1);
        items.swap(i, j);
    }
}

fn find_list_items_in_avatar_stack(value: &Value) -> Option<&[Value]> {
    let path = "/rendererContext/commandContext/onTap/innertubeCommand/showDialogCommand/\
        panelLoadingStrategy/inlineContent/dialogViewModel/customContent/listViewModel/listItems";
    if let Some(items) = value.pointer(path).and_then(Value::as_array) {
        return Some(items.as_slice());
    }

    fn find_list_items(value: &Value) -> Option<&[Value]> {
        match value {
            Value::Array(values) => values.iter().find_map(find_list_items),
            Value::Object(map) => {
                if let Some(items) = map
                    .get("listViewModel")
                    .and_then(|list| list.get("listItems"))
                    .and_then(Value::as_array)
                {
                    return Some(items.as_slice());
                }
                map.values().find_map(find_list_items)
            }
            _ => None,
        }
    }

    find_list_items(value)
}

fn find_collaboration_items(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(values) => values.iter().find_map(find_collaboration_items),
        Value::Object(map) => {
            if let Some(stack) = map.get("avatarStackViewModel") {
                if let Some(items) = find_list_items_in_avatar_stack(stack) {
                    return Some(items);
                }
            }
            map.values().find_map(find_collaboration_items)
        }
        _ => None,
    }
}

fn parse_video_collaborators(lockup: &Value) -> Vec<VideoCollaborator> {
    let Some(items) = find_collaboration_items(lockup) else {
        return Vec::new();
    };

    let mut collaborators = Vec::new();
    let mut seen_ids = HashSet::new();
    for item in items {
        let Some(view) = item.get("listItemViewModel") else {
            continue;
        };
        let name = extract_yt_text(view.get("title")).unwrap_or_default();
        let browse_id = view
            .pointer("/rendererContext/commandContext/onTap/innertubeCommand/browseEndpoint/browseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = if !browse_id.is_empty() {
            browse_id
        } else {
            view.pointer("/rendererContext/commandContext/onTap/innertubeCommand/commandMetadata/webCommandMetadata/url")
                .and_then(Value::as_str)
                .and_then(|url| url.strip_prefix("/channel/"))
                .unwrap_or_default()
                .to_string()
        };
        if name.is_empty() && id.is_empty() {
            continue;
        }
        if !id.is_empty() && !seen_ids.insert(id.clone()) {
            continue;
        }

        let subtitle = extract_yt_text(view.get("subtitle")).unwrap_or_default();
        let subtitle: String = subtitle
            .chars()
            .filter(|ch| !matches!(*ch, '\u{200e}' | '\u{200f}' | '\u{2066}' | '\u{2067}' | '\u{2068}' | '\u{2069}'))
            .collect();
        let metadata_parts: Vec<&str> = subtitle
            .split(['•', '·'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        let handle = metadata_parts
            .iter()
            .find(|part| part.starts_with('@'))
            .copied()
            .unwrap_or_default()
            .to_string();
        let subscriber_count_text = metadata_parts
            .iter()
            .find(|part| part.to_ascii_lowercase().contains("subscriber"))
            .copied()
            .unwrap_or_default()
            .to_string();
        let avatar_url = pick_json_sources_url(
            view.pointer("/leadingAccessory/avatarViewModel/image/sources")
                .and_then(Value::as_array),
            48,
        )
        .unwrap_or_default();

        collaborators.push(VideoCollaborator {
            id,
            name,
            handle,
            subscriber_count_text,
            avatar_url,
        });
    }
    collaborators
}

fn parse_video_lockup_json(lockup: &Value) -> Option<VideoSummary> {
    let content_type = lockup
        .get("contentType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if content_type == "LOCKUP_CONTENT_TYPE_PLAYLIST"
        || content_type == "LOCKUP_CONTENT_TYPE_PODCAST"
        || content_type == "LOCKUP_CONTENT_TYPE_ALBUM"
        || content_type == "LOCKUP_CONTENT_TYPE_CHANNEL"
    {
        return None;
    }

    let vid = lockup
        .get("contentId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| {
            s.len() == 11
                && !s.starts_with("PL")
                && !s.starts_with("OL")
                && !s.starts_with("RD")
                && !s.starts_with("UU")
        })
        .or_else(|| {
            lockup
                .pointer("/rendererContext/commandContext/onTap/innertubeCommand/watchEndpoint/videoId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            lockup
                .pointer("/rendererContext/commandContext/onTap/innertubeCommand/reelWatchEndpoint/videoId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })?;

    let title = lockup
        .pointer("/metadata/lockupMetadataViewModel/title/content")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| extract_yt_text(lockup.pointer("/metadata/lockupMetadataViewModel/title")))?;

    if title == "[Private video]" || title == "[Deleted video]" {
        return None;
    }

    let mut is_short = lockup
        .pointer("/rendererContext/commandContext/onTap/innertubeCommand/reelWatchEndpoint")
        .is_some();
    let mut duration_text = String::new();

    if let Some(overlays) = lockup
        .pointer("/contentImage/thumbnailViewModel/overlays")
        .and_then(|v| v.as_array())
    {
        for ov in overlays {
            let badge_arrays = [
                ov.pointer("/thumbnailBottomOverlayViewModel/badges")
                    .and_then(|v| v.as_array()),
                ov.pointer("/thumbnailOverlayBadgeViewModel/thumbnailBadges")
                    .and_then(|v| v.as_array()),
            ];
            for badges in badge_arrays.into_iter().flatten() {
                for b in badges {
                    let Some(vm) = b.get("thumbnailBadgeViewModel") else {
                        continue;
                    };
                    if let Some(txt) = vm.get("text").and_then(|v| v.as_str()).map(str::trim) {
                        if txt.eq_ignore_ascii_case("SHORTS") {
                            is_short = true;
                        } else if duration_text.is_empty()
                            && txt.chars().any(|c| c.is_ascii_digit())
                        {
                            duration_text = txt.to_string();
                        }
                    }
                }
            }
        }
    }
    let duration_seconds = parse_mmss_to_seconds(&duration_text);

    let collaborators = parse_video_collaborators(lockup);
    let mut channel_name = String::new();
    let mut channel_id = lockup
        .pointer("/metadata/lockupMetadataViewModel/image/decoratedAvatarViewModel/rendererContext/commandContext/onTap/innertubeCommand/browseEndpoint/browseId")
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("UC"))
        .unwrap_or_default()
        .to_string();
    let mut channel_avatar_url = pick_json_sources_url(
        lockup
            .pointer("/metadata/lockupMetadataViewModel/image/decoratedAvatarViewModel/avatar/avatarViewModel/image/sources")
            .and_then(|v| v.as_array()),
        64,
    )
    .unwrap_or_default();
    if let Some(primary) = collaborators.first() {
        if !primary.id.is_empty() {
            channel_id = primary.id.clone();
        }
        if !primary.name.is_empty() {
            channel_name = primary.name.clone();
        }
        if !primary.avatar_url.is_empty() {
            channel_avatar_url = primary.avatar_url.clone();
        }
    }

    let mut view_count_text = String::new();
    let mut published_text = String::new();

    if let Some(rows) = lockup
        .pointer("/metadata/lockupMetadataViewModel/metadata/contentMetadataViewModel/metadataRows")
        .and_then(|v| v.as_array())
    {
        for row in rows {
            let Some(parts) = row.get("metadataParts").and_then(|v| v.as_array()) else {
                continue;
            };
            for part in parts {
                let Some(txt) = part
                    .pointer("/text/content")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                if channel_id.is_empty() {
                    if let Some(bid) = part
                        .pointer("/text/commandRuns/0/onTap/innertubeCommand/browseEndpoint/browseId")
                        .and_then(|v| v.as_str())
                        .filter(|s| s.starts_with("UC"))
                    {
                        channel_id = bid.to_string();
                        if channel_name.is_empty() {
                            channel_name = txt.to_string();
                            continue;
                        }
                    }
                }
                let lower = txt.to_ascii_lowercase();
                if view_count_text.is_empty()
                    && (lower.contains("view") || lower.contains("watching"))
                {
                    view_count_text = txt.to_string();
                } else if published_text.is_empty()
                    && (lower.contains("ago")
                        || lower.contains("streamed")
                        || lower.contains("premiered"))
                {
                    published_text = txt.to_string();
                } else if channel_name.is_empty()
                    && !lower.contains("episode")
                    && !lower.contains("lesson")
                    && !lower.contains("updated")
                    && txt != "•"
                    && txt != "·"
                {
                    channel_name = txt.to_string();
                }
            }
        }
    }

    let view_count = parse_compact_count_str(&view_count_text);
    let thumbnail_url = pick_json_sources_url(
        lockup
            .pointer("/contentImage/thumbnailViewModel/image/sources")
            .and_then(|v| v.as_array()),
        320,
    )
    .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{vid}/mqdefault.jpg"));

    Some(VideoSummary {
        id: vid.to_string(),
        title,
        channel_id,
        channel_name,
        channel_avatar_url,
        duration_seconds,
        duration_text,
        view_count_text,
        view_count,
        published_unix: inferred_publish_timestamp(&published_text),
        published_text,
        is_short,
        thumbnail_url,
        description_snippet: String::new(),
        collaborators,
    })
}

fn fill_missing_channel_metadata(target: &mut [VideoSummary], source: &[VideoSummary]) {
    let by_id: HashMap<&str, &VideoSummary> = source
        .iter()
        .map(|video| (video.id.as_str(), video))
        .collect();
    for video in target {
        let Some(details) = by_id.get(video.id.as_str()) else {
            continue;
        };
        if video.duration_seconds.is_none() {
            video.duration_seconds = details.duration_seconds;
        }
        if video.duration_text.is_empty() {
            video.duration_text = if !details.duration_text.is_empty() {
                details.duration_text.clone()
            } else {
                video
                    .duration_seconds
                    .map(format_duration)
                    .unwrap_or_default()
            };
        }
        if video.view_count.is_none() {
            video.view_count = details.view_count;
        }
        if video.view_count_text.is_empty() {
            video.view_count_text = details.view_count_text.clone();
        }
        if video.published_unix.is_none() {
            video.published_unix = details.published_unix;
        }
        if video.published_text.is_empty() {
            video.published_text = details.published_text.clone();
        }
        if video.channel_id.is_empty() {
            video.channel_id = details.channel_id.clone();
        }
        if video.channel_name.is_empty() {
            video.channel_name = details.channel_name.clone();
        }
        if video.channel_avatar_url.is_empty() {
            video.channel_avatar_url = details.channel_avatar_url.clone();
        }
        if video.collaborators.is_empty() && !details.collaborators.is_empty() {
            video.collaborators = details.collaborators.clone();
        }
    }
}

fn parse_home_feed_response(val: &Value) -> PagedVideos {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut ctoken = None;
    if let Some(contents) = val.get("contents") {
        collect_playlist_tracks_recursive(contents, &mut items, &mut seen, &mut ctoken);
    }
    if let Some(actions) = val.get("onResponseReceivedActions") {
        collect_playlist_tracks_recursive(actions, &mut items, &mut seen, &mut ctoken);
    }
    let next = ctoken.map(|tok| PageCursor {
        ctoken: tok,
        endpoint: ContinuationEndpoint::Browse,
        visitor_data: None,
        kind: CursorKind::DirectHomeFeed,
    });
    PagedVideos { items, next }
}

fn collect_playlist_tracks_recursive(
    node: &Value,
    out_videos: &mut Vec<VideoSummary>,
    seen_ids: &mut HashSet<String>,
    out_ctoken: &mut Option<String>,
) {
    match node {
        Value::Array(arr) => {
            for item in arr {
                collect_playlist_tracks_recursive(item, out_videos, seen_ids, out_ctoken);
            }
        }
        Value::Object(map) => {
            if let Some(pvr) = map
                .get("playlistVideoRenderer")
                .or_else(|| map.get("playlistPanelVideoRenderer"))
            {
                if let Some(v) = parse_playlist_video_renderer(pvr) {
                    if seen_ids.insert(v.id.clone()) {
                        out_videos.push(v);
                    }
                }
                return;
            }
            if let Some(lockup) = map.get("lockupViewModel") {
                if let Some(v) = parse_video_lockup_json(lockup) {
                    if seen_ids.insert(v.id.clone()) {
                        out_videos.push(v);
                    }
                }
                return;
            }
            if let Some(vr) = map
                .get("videoRenderer")
                .or_else(|| map.get("gridVideoRenderer"))
            {
                if let Some(v) = parse_video_renderer_json(vr) {
                    if seen_ids.insert(v.id.clone()) {
                        out_videos.push(v);
                    }
                }
                return;
            }
            if let Some(sl) = map.get("shortsLockupViewModel") {
                if let Some(v) = parse_short_lockup_item(sl, "", None) {
                    if seen_ids.insert(v.id.clone()) {
                        out_videos.push(v);
                    }
                }
                return;
            }
            if let Some(reel) = map.get("reelItemRenderer") {
                if let Some(v) = parse_reel_item(reel, "", None) {
                    if seen_ids.insert(v.id.clone()) {
                        out_videos.push(v);
                    }
                }
                return;
            }
            if let Some(cont) = map.get("continuationItemRenderer") {
                if let Some(tok) = extract_continuation_token_from_renderer(cont) {
                    *out_ctoken = Some(tok);
                }
                return;
            }
            if let Some(cvm) = map.get("continuationItemViewModel") {
                if let Some(tok) = cvm
                    .pointer("/continuationCommand/innertubeCommand/continuationCommand/token")
                    .or_else(|| cvm.pointer("/continuationCommand/token"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    *out_ctoken = Some(tok.to_string());
                }
                return;
            }
            for (k, v) in map {
                // Avoid descending into unrelated watch-next / sidebar recommendations
                if k == "secondaryResults"
                    || k == "playerOverlays"
                    || k == "header"
                    || k == "topbar"
                {
                    continue;
                }
                collect_playlist_tracks_recursive(v, out_videos, seen_ids, out_ctoken);
            }
        }
        _ => {}
    }
}

fn parse_playlist_browse_response(val: &Value, playlist_id: &str) -> Option<RemotePlaylist> {
    let mut videos = Vec::new();
    let mut seen = HashSet::new();
    let mut ctoken = None;

    if let Some(contents) = val.get("contents") {
        collect_playlist_tracks_recursive(contents, &mut videos, &mut seen, &mut ctoken);
    }
    if let Some(actions) = val.get("onResponseReceivedActions") {
        collect_playlist_tracks_recursive(actions, &mut videos, &mut seen, &mut ctoken);
    }
    if let Some(cont_contents) = val.get("continuationContents") {
        collect_playlist_tracks_recursive(cont_contents, &mut videos, &mut seen, &mut ctoken);
    }

    let name = val
        .pointer("/header/pageHeaderRenderer/content/pageHeaderViewModel/title/dynamicTextViewModel/text/content")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            val.pointer("/header/pageHeaderRenderer/pageTitle")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| extract_yt_text(val.pointer("/header/playlistHeaderRenderer/title")))
        .or_else(|| {
            extract_yt_text(val.pointer(
                "/sidebar/playlistSidebarRenderer/items/0/playlistSidebarPrimaryInfoRenderer/title",
            ))
        })
        .or_else(|| {
            val.pointer("/microformat/microformatDataRenderer/title")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();

    if name.is_empty() && videos.is_empty() {
        return None;
    }

    let mut channel_name = String::new();
    let mut video_count_text = String::new();

    if let Some(rows) = val
        .pointer("/header/pageHeaderRenderer/content/pageHeaderViewModel/metadata/contentMetadataViewModel/metadataRows")
        .and_then(|v| v.as_array())
    {
        for row in rows {
            if let Some(parts) = row.get("metadataParts").and_then(|v| v.as_array()) {
                for part in parts {
                    if let Some(av_txt) = part
                        .pointer("/avatarStack/avatarStackViewModel/text/content")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        if channel_name.is_empty() {
                            channel_name = av_txt
                                .strip_prefix("by ")
                                .unwrap_or(av_txt)
                                .trim()
                                .to_string();
                        }
                    }
                    if let Some(txt) = part
                        .pointer("/text/content")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        let lower = txt.to_ascii_lowercase();
                        if video_count_text.is_empty()
                            && (lower.contains("video")
                                || lower.contains("song")
                                || lower.contains("track"))
                        {
                            video_count_text = txt.to_string();
                        } else if channel_name.is_empty()
                            && !lower.contains("view")
                            && !lower.contains("playlist")
                            && !lower.contains("updated")
                        {
                            channel_name = txt
                                .strip_prefix("by ")
                                .unwrap_or(txt)
                                .trim()
                                .to_string();
                        }
                    }
                }
            }
        }
    }

    if channel_name.is_empty() {
        channel_name = extract_yt_text(val.pointer("/header/playlistHeaderRenderer/ownerText"))
            .or_else(|| {
                extract_yt_text(val.pointer(
                    "/sidebar/playlistSidebarRenderer/items/1/playlistSidebarSecondaryInfoRenderer/videoOwner/videoOwnerRenderer/title",
                ))
            })
            .or_else(|| {
                videos
                    .first()
                    .map(|v| v.channel_name.clone())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
    }

    if video_count_text.is_empty() {
        video_count_text =
            extract_yt_text(val.pointer("/header/playlistHeaderRenderer/numVideosText"))
                .or_else(|| {
                    extract_yt_text(val.pointer(
                        "/sidebar/playlistSidebarRenderer/items/0/playlistSidebarPrimaryInfoRenderer/stats/0",
                    ))
                })
                .unwrap_or_else(|| format!("{} videos", videos.len()));
    }

    let next = ctoken.map(|tok| PageCursor {
        ctoken: tok,
        endpoint: ContinuationEndpoint::Browse,
        visitor_data: None,
        kind: CursorKind::DirectPlaylistVideos,
    });

    Some(RemotePlaylist {
        id: playlist_id.to_string(),
        name: if name.is_empty() {
            "Playlist".to_string()
        } else {
            name
        },
        channel_name,
        video_count_text,
        videos,
        next,
    })
}

fn parse_playlist_continuation_response(val: &Value) -> PagedVideos {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut ctoken = None;
    if let Some(actions) = val.get("onResponseReceivedActions") {
        collect_playlist_tracks_recursive(actions, &mut items, &mut seen, &mut ctoken);
    }
    if let Some(cont_contents) = val.get("continuationContents") {
        collect_playlist_tracks_recursive(cont_contents, &mut items, &mut seen, &mut ctoken);
    }
    let next = ctoken.map(|tok| PageCursor {
        ctoken: tok,
        endpoint: ContinuationEndpoint::Browse,
        visitor_data: None,
        kind: CursorKind::DirectPlaylistVideos,
    });
    PagedVideos { items, next }
}

fn collect_channel_playlists_recursive(
    node: &Value,
    fallback_channel_name: &str,
    out: &mut Vec<PlaylistSearchSummary>,
    seen: &mut HashSet<String>,
    out_ctoken: &mut Option<String>,
) {
    match node {
        Value::Array(arr) => {
            for it in arr {
                collect_channel_playlists_recursive(it, fallback_channel_name, out, seen, out_ctoken);
            }
        }
        Value::Object(map) => {
            if let Some(lockup) = map.get("lockupViewModel") {
                if let Some(pl) = parse_playlist_lockup_json(lockup, fallback_channel_name) {
                    if seen.insert(pl.id.clone()) {
                        out.push(pl);
                    }
                }
                return;
            }
            for key in ["playlistRenderer", "gridPlaylistRenderer", "compactPlaylistRenderer"] {
                if let Some(pr) = map.get(key) {
                    if let Some(pl) = parse_playlist_renderer_json(pr, fallback_channel_name) {
                        if seen.insert(pl.id.clone()) {
                            out.push(pl);
                        }
                    }
                    return;
                }
            }
            if let Some(cont) = map.get("continuationItemRenderer") {
                if let Some(tok) = extract_continuation_token_from_renderer(cont) {
                    *out_ctoken = Some(tok);
                }
                return;
            }
            for (k, v) in map {
                if k == "header" || k == "topbar" || k == "microformat" {
                    continue;
                }
                collect_channel_playlists_recursive(v, fallback_channel_name, out, seen, out_ctoken);
            }
        }
        _ => {}
    }
}

fn parse_channel_playlists_browse_response(
    val: &Value,
    fallback_channel_name: &str,
) -> PagedPlaylists {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut ctoken = None;

    if let Some(tabs) = val
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs")
        .and_then(|v| v.as_array())
    {
        if let Some(pl_tab) = tabs
            .iter()
            .filter_map(|t| t.get("tabRenderer"))
            .find(|tr| {
                tr.pointer("/endpoint/commandMetadata/webCommandMetadata/url")
                    .and_then(|v| v.as_str())
                    .map(|u| u.ends_with("/playlists"))
                    .unwrap_or(false)
            })
            .or_else(|| {
                tabs.iter()
                    .filter_map(|t| t.get("tabRenderer"))
                    .find(|tr| tr.get("selected").and_then(|v| v.as_bool()) == Some(true))
            })
        {
            if let Some(content) = pl_tab.get("content") {
                collect_channel_playlists_recursive(
                    content,
                    fallback_channel_name,
                    &mut items,
                    &mut seen,
                    &mut ctoken,
                );
            }
        }
    }

    if let Some(actions) = val.get("onResponseReceivedActions") {
        collect_channel_playlists_recursive(
            actions,
            fallback_channel_name,
            &mut items,
            &mut seen,
            &mut ctoken,
        );
    }

    let next = ctoken.map(|tok| PageCursor {
        ctoken: tok,
        endpoint: ContinuationEndpoint::Browse,
        visitor_data: None,
        kind: CursorKind::DirectChannelPlaylists,
    });

    PagedPlaylists { items, next }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectSearchMode {
    All,
    ShortsOnly,
    PlaylistsOnly,
}

fn push_proto_varint_field(out: &mut Vec<u8>, field_number: u64, val: u64) {
    push_proto_varint(out, field_number << 3);
    push_proto_varint(out, val);
}

fn encode_direct_search_params(
    sort: SearchSortOrder,
    date: UploadDateFilter,
    duration: DurationFilter,
    search_type: Option<u64>,
) -> String {
    let mut filters = Vec::with_capacity(8);
    if let Some(d) = match date {
        UploadDateFilter::AnyTime => None,
        UploadDateFilter::Today => Some(2),
        UploadDateFilter::ThisWeek => Some(3),
        UploadDateFilter::ThisMonth => Some(4),
        UploadDateFilter::ThisYear => Some(5),
    } {
        push_proto_varint_field(&mut filters, 1, d);
    }
    if let Some(st) = search_type {
        push_proto_varint_field(&mut filters, 2, st);
    }
    if let Some(dur) = match duration {
        DurationFilter::AnyLength => None,
        DurationFilter::Under4Min => Some(1),
        DurationFilter::FourTo20Min => Some(3),
        DurationFilter::Over20Min => Some(2),
    } {
        push_proto_varint_field(&mut filters, 3, dur);
    }

    let mut outer = Vec::with_capacity(16);
    if let Some(s) = match sort {
        SearchSortOrder::Relevance => None,
        SearchSortOrder::UploadDate => Some(2),
        SearchSortOrder::ViewCount => Some(3),
    } {
        push_proto_varint_field(&mut outer, 1, s);
    }
    if !filters.is_empty() {
        push_proto_bytes_field(&mut outer, 2, &filters);
    }
    // Disable sensitive-topic blocking so legitimate queries never return an empty promo screen
    push_proto_varint_field(&mut outer, 30, 1);

    encode_base64_url_safe_pct(&outer)
}

fn extract_search_header_shorts_chip(val: &Value) -> Option<(Option<String>, Option<String>)> {
    let mut chip_arrays: Vec<&Vec<Value>> = Vec::new();
    if let Some(arr) = val
        .pointer("/header/searchHeaderRenderer/chipBar/chipCloudRenderer/chips")
        .and_then(|v| v.as_array())
    {
        chip_arrays.push(arr);
    }
    if let Some(cmds) = val.get("onResponseReceivedCommands").and_then(|v| v.as_array()) {
        for cmd in cmds {
            if let Some(arr) = cmd
                .pointer("/reloadContinuationItemsCommand/continuationItems")
                .and_then(|v| v.as_array())
            {
                for item in arr {
                    if let Some(chips) = item
                        .pointer("/searchHeaderRenderer/chipBar/chipCloudRenderer/chips")
                        .and_then(|v| v.as_array())
                    {
                        chip_arrays.push(chips);
                    }
                }
            }
        }
    }

    for chips in chip_arrays {
        for entry in chips {
            if let Some(chip) = entry.get("chipCloudChipRenderer") {
                let label = extract_yt_text(chip.get("text")).unwrap_or_default();
                if label.trim().eq_ignore_ascii_case("shorts") {
                    let ctok = chip
                        .pointer("/navigationEndpoint/continuationCommand/token")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    let params = chip
                        .pointer("/navigationEndpoint/searchEndpoint/params")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    if ctok.is_some() || params.is_some() {
                        return Some((ctok, params));
                    }
                }
            }
        }
    }
    None
}

fn push_short_summary_if_unseen(
    mut v: VideoSummary,
    out_items: &mut Vec<SearchItem>,
    seen_keys: &mut HashSet<String>,
) {
    v.is_short = true;
    if v.duration_text.is_empty() {
        v.duration_text = "SHORTS".to_string();
    }
    if seen_keys.insert(format!("vid:{}", v.id)) {
        out_items.push(SearchItem::Video(v));
    }
}

fn collect_search_entries_from_array(
    arr: &[Value],
    mode: DirectSearchMode,
    out_items: &mut Vec<SearchItem>,
    seen_keys: &mut HashSet<String>,
    out_ctoken: &mut Option<String>,
) {
    for entry in arr {
        if let Some(sub) = entry
            .pointer("/itemSectionRenderer/contents")
            .or_else(|| entry.pointer("/sectionListRenderer/contents"))
            .and_then(|v| v.as_array())
        {
            collect_search_entries_from_array(sub, mode, out_items, seen_keys, out_ctoken);
            continue;
        }
        if let Some(cont) = entry.get("continuationItemRenderer") {
            if let Some(tok) = extract_continuation_token_from_renderer(cont) {
                *out_ctoken = Some(tok);
            }
            continue;
        }

        let inner = entry
            .pointer("/richItemRenderer/content")
            .unwrap_or(entry);

        // Unpack Shorts shelves (reelShelfRenderer & gridShelfViewModel) and standalone Short items
        if mode != DirectSearchMode::PlaylistsOnly {
            if let Some(sl) = inner.get("shortsLockupViewModel") {
                if let Some(v) = parse_short_lockup_item(sl, "", None) {
                    push_short_summary_if_unseen(v, out_items, seen_keys);
                }
                continue;
            }
            if let Some(reel) = inner.get("reelItemRenderer") {
                if let Some(v) = parse_reel_item(reel, "", None) {
                    push_short_summary_if_unseen(v, out_items, seen_keys);
                }
                continue;
            }
            if let Some(reel_items) = inner
                .pointer("/reelShelfRenderer/items")
                .and_then(|v| v.as_array())
            {
                collect_search_entries_from_array(
                    reel_items, mode, out_items, seen_keys, out_ctoken,
                );
                continue;
            }
            if let Some(grid_items) = inner
                .pointer("/gridShelfViewModel/contents")
                .and_then(|v| v.as_array())
            {
                collect_search_entries_from_array(
                    grid_items, mode, out_items, seen_keys, out_ctoken,
                );
                continue;
            }
        }

        if let Some(lockup) = inner.get("lockupViewModel") {
            if mode != DirectSearchMode::ShortsOnly {
                if let Some(pl) = parse_playlist_lockup_json(lockup, "") {
                    if seen_keys.insert(format!("pl:{}", pl.id)) {
                        out_items.push(SearchItem::Playlist(pl));
                    }
                    continue;
                }
            }
            if mode != DirectSearchMode::PlaylistsOnly {
                if let Some(mut v) = parse_video_lockup_json(lockup) {
                    if mode == DirectSearchMode::ShortsOnly {
                        let short_like = v.is_short
                            || v.duration_seconds.map(|d| d > 0 && d <= 180).unwrap_or(false);
                        if short_like {
                            push_short_summary_if_unseen(v, out_items, seen_keys);
                        }
                    } else if seen_keys.insert(format!("vid:{}", v.id)) {
                        if v.is_short && v.duration_text.is_empty() {
                            v.duration_text = "SHORTS".to_string();
                        }
                        out_items.push(SearchItem::Video(v));
                    }
                    continue;
                }
            }
        }

        if mode != DirectSearchMode::ShortsOnly {
            if let Some(pr) = inner
                .get("playlistRenderer")
                .or_else(|| inner.get("compactPlaylistRenderer"))
            {
                if let Some(pl) = parse_playlist_renderer_json(pr, "") {
                    if seen_keys.insert(format!("pl:{}", pl.id)) {
                        out_items.push(SearchItem::Playlist(pl));
                    }
                    continue;
                }
            }
        }

        if mode == DirectSearchMode::PlaylistsOnly {
            continue;
        }

        if let Some(vr) = inner.get("videoRenderer") {
            if let Some(mut v) = parse_video_renderer_json(vr) {
                if mode == DirectSearchMode::ShortsOnly {
                    let short_like = v.is_short
                        || v.duration_seconds.map(|d| d > 0 && d <= 180).unwrap_or(false);
                    if short_like {
                        push_short_summary_if_unseen(v, out_items, seen_keys);
                    }
                } else if seen_keys.insert(format!("vid:{}", v.id)) {
                    if v.is_short && v.duration_text.is_empty() {
                        v.duration_text = "SHORTS".to_string();
                    }
                    out_items.push(SearchItem::Video(v));
                }
            }
        } else if mode == DirectSearchMode::All {
            if let Some(cr) = inner.get("channelRenderer") {
                if let Some(c) = parse_channel_renderer_json(cr) {
                    if seen_keys.insert(format!("ch:{}", c.id)) {
                        out_items.push(SearchItem::Channel(c));
                    }
                }
            }
        }

        if let Some(shelf_items) = inner
            .pointer("/shelfRenderer/content/verticalListRenderer/items")
            .or_else(|| inner.pointer("/shelfRenderer/content/horizontalListRenderer/items"))
            .and_then(|v| v.as_array())
        {
            collect_search_entries_from_array(shelf_items, mode, out_items, seen_keys, out_ctoken);
        }
    }
}

fn parse_direct_search_response(val: &Value, mode: DirectSearchMode) -> PagedSearchItems {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut ctoken = None;

    if let Some(sections) = val
        .pointer("/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents")
        .and_then(|v| v.as_array())
    {
        collect_search_entries_from_array(sections, mode, &mut items, &mut seen, &mut ctoken);
    }

    for key in ["onResponseReceivedCommands", "onResponseReceivedActions"] {
        if let Some(cmds) = val.get(key).and_then(|v| v.as_array()) {
            for cmd in cmds {
                if let Some(arr) = cmd
                    .pointer("/appendContinuationItemsAction/continuationItems")
                    .and_then(|v| v.as_array())
                {
                    collect_search_entries_from_array(
                        arr, mode, &mut items, &mut seen, &mut ctoken,
                    );
                }
                if let Some(reload) = cmd.get("reloadContinuationItemsCommand") {
                    let is_header = reload
                        .get("slot")
                        .and_then(|v| v.as_str())
                        .map(|s| s.contains("HEADER"))
                        .unwrap_or(false);
                    if !is_header {
                        if let Some(arr) = reload
                            .get("continuationItems")
                            .and_then(|v| v.as_array())
                        {
                            collect_search_entries_from_array(
                                arr, mode, &mut items, &mut seen, &mut ctoken,
                            );
                        }
                    }
                }
            }
        }
    }

    let next = ctoken.map(|tok| PageCursor {
        ctoken: tok,
        endpoint: ContinuationEndpoint::Search,
        visitor_data: None,
        kind: CursorKind::DirectSearch,
    });

    PagedSearchItems { items, next }
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

    let thumbnail_url = pick_thumbnail_url(&item.thumbnail, 320);

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
    let mut published_text = item.publish_date_txt.unwrap_or_default();
    let published_unix = item
        .publish_date
        .map(|dt| dt.unix_timestamp())
        .or_else(|| inferred_publish_timestamp(&published_text));
    if published_text.is_empty() {
        if let Some(timestamp) = published_unix {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(timestamp);
            published_text = format_relative_published_text(timestamp, now);
        }
    }
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
        collaborators: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duration_test_video(id: &str, seconds: Option<u64>, text: &str) -> VideoSummary {
        VideoSummary {
            id: id.to_string(),
            title: "test".to_string(),
            channel_id: String::new(),
            channel_name: String::new(),
            channel_avatar_url: String::new(),
            duration_seconds: seconds,
            duration_text: text.to_string(),
            view_count_text: String::new(),
            view_count: None,
            published_text: String::new(),
            published_unix: None,
            is_short: false,
            thumbnail_url: String::new(),
            description_snippet: String::new(),
            collaborators: Vec::new(),
        }
    }

    #[test]
    fn test_channel_renderer_parses_duration_badge() {
        let renderer = json!({
            "videoId": "video123",
            "title": { "simpleText": "Channel upload" },
            "lengthText": { "simpleText": "12:34" }
        });
        let parsed = parse_video_renderer_json(&renderer).unwrap();
        assert_eq!(parsed.duration_seconds, Some(754));
        assert_eq!(parsed.duration_text, "12:34");
        assert!(!parsed.is_collaboration());
    }

    #[test]
    fn test_video_lockup_parses_all_collaboration_channels() {
        let collaborator = |id: &str, name: &str, handle: &str, subs: &str, avatar: &str| {
            json!({
                "listItemViewModel": {
                    "title": { "content": name },
                    "subtitle": { "content": format!("\u{200e}\u{2068}{handle}\u{2069} • \u{2068}{subs}\u{2069}") },
                    "leadingAccessory": {
                        "avatarViewModel": {
                            "image": { "sources": [{ "url": avatar, "width": 68, "height": 68 }] }
                        }
                    },
                    "rendererContext": {
                        "commandContext": {
                            "onTap": {
                                "innertubeCommand": {
                                    "browseEndpoint": { "browseId": id }
                                }
                            }
                        }
                    }
                }
            })
        };
        let lockup = json!({
            "contentId": "abc123XYZ89",
            "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
            "metadata": {
                "lockupMetadataViewModel": {
                    "title": { "content": "A collaboration video" },
                    "image": {
                        "avatarStackViewModel": {
                            "rendererContext": {
                                "commandContext": {
                                    "onTap": {
                                        "innertubeCommand": {
                                            "showDialogCommand": {
                                                "panelLoadingStrategy": {
                                                    "inlineContent": {
                                                        "dialogViewModel": {
                                                            "customContent": {
                                                                "listViewModel": {
                                                                    "listItems": [
                                                                        collaborator("UC-one", "Channel One", "@one", "1.2M subscribers", "https://img/one.jpg"),
                                                                        collaborator("UC-two", "Channel Two", "@two", "345K subscribers", "https://img/two.jpg"),
                                                                        collaborator("UC-three", "Channel Three", "@three", "89K subscribers", "https://img/three.jpg")
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "metadata": {
                        "contentMetadataViewModel": {
                            "metadataRows": [{
                                "metadataParts": [
                                    { "text": { "content": "Channel One" } },
                                    { "text": { "content": "and 2 more" } }
                                ]
                            }]
                        }
                    }
                }
            }
        });

        let parsed = parse_video_lockup_json(&lockup).unwrap();
        assert_eq!(parsed.collaborators.len(), 3);
        assert_eq!(parsed.channel_id, "UC-one");
        assert_eq!(parsed.channel_name, "Channel One");
        assert_eq!(parsed.channel_avatar_url, "https://img/one.jpg");
        assert_eq!(parsed.collaborators[1].handle, "@two");
        assert_eq!(parsed.collaborators[1].subscriber_count_text, "345K subscribers");
        assert_eq!(parsed.channel_byline().as_deref(), Some("Channel One and 2 more"));
    }

    #[test]
    fn test_video_renderer_finds_nested_collaboration_avatar_stack() {
        let list_item = |id: &str, name: &str| {
            json!({
                "listItemViewModel": {
                    "title": { "content": name },
                    "rendererContext": {
                        "commandContext": {
                            "onTap": {
                                "innertubeCommand": {
                                    "browseEndpoint": { "browseId": id }
                                }
                            }
                        }
                    }
                }
            })
        };
        let renderer = json!({
            "videoId": "video-collab",
            "ownerText": { "simpleText": "Lead channel" },
            "newRendererWrapper": {
                "metadata": {
                    "image": {
                        "avatarStackViewModel": {
                            "rendererContext": {
                                "commandContext": {
                                    "onTap": {
                                        "innertubeCommand": {
                                            "showDialogCommand": {
                                                "panelLoadingStrategy": {
                                                    "inlineContent": {
                                                        "dialogViewModel": {
                                                            "customContent": {
                                                                "listViewModel": {
                                                                    "listItems": [
                                                                        list_item("UC-first", "First channel"),
                                                                        list_item("UC-second", "Second channel")
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        let parsed = parse_video_renderer_json(&renderer).unwrap();
        assert!(parsed.is_collaboration());
        assert_eq!(parsed.collaborators.len(), 2);
        assert_eq!(parsed.channel_name, "First channel");
        assert_eq!(parsed.channel_id, "UC-first");
    }

    #[test]
    fn test_channel_duration_fallback_fills_only_missing_metadata() {
        let mut from_rustypipe = vec![
            duration_test_video("missing", None, ""),
            duration_test_video("known", Some(60), "1:00"),
        ];
        let mut direct = vec![
            duration_test_video("missing", Some(754), "12:34"),
            duration_test_video("known", Some(99), "1:39"),
        ];
        direct[0].channel_id = "UC-primary".to_string();
        direct[0].channel_name = "Primary Channel".to_string();
        direct[0].channel_avatar_url = "https://img/primary.jpg".to_string();
        direct[0].collaborators = vec![
            VideoCollaborator {
                id: "UC-primary".to_string(),
                name: "Primary Channel".to_string(),
                ..VideoCollaborator::default()
            },
            VideoCollaborator {
                id: "UC-second".to_string(),
                name: "Second Channel".to_string(),
                ..VideoCollaborator::default()
            },
        ];
        fill_missing_channel_metadata(&mut from_rustypipe, &direct);
        assert_eq!(from_rustypipe[0].duration_seconds, Some(754));
        assert_eq!(from_rustypipe[0].duration_text, "12:34");
        assert_eq!(from_rustypipe[0].channel_id, "UC-primary");
        assert_eq!(from_rustypipe[0].channel_name, "Primary Channel");
        assert_eq!(from_rustypipe[0].channel_avatar_url, "https://img/primary.jpg");
        assert_eq!(from_rustypipe[0].collaborators.len(), 2);
        assert_eq!(from_rustypipe[1].duration_seconds, Some(60));
        assert_eq!(from_rustypipe[1].duration_text, "1:00");
    }

    #[test]
    fn test_hardware_friendly_codec_selection() {
        let pool = CandidatePool {
            muxed: vec![RawVideoCandidate {
                itag: 18,
                url: "https://cdn.example/360_muxed.mp4".to_string(),
                width: 640,
                height: 360,
                fps: 30,
                bitrate: 500_000,
                filesize: Some(11_250_000),
                hdr: false,
                is_mp4: true,
                codec: SimpleVideoCodec::H264,
            }],
            adaptive_video: vec![
                RawVideoCandidate {
                    itag: 398,
                    url: "https://cdn.example/720_av1.mp4".to_string(),
                    width: 1280,
                    height: 720,
                    fps: 30,
                    bitrate: 900_000,
                    filesize: Some(20_000_000),
                    hdr: false,
                    is_mp4: true,
                    codec: SimpleVideoCodec::Av1,
                },
                RawVideoCandidate {
                    itag: 136,
                    url: "https://cdn.example/720_h264.mp4".to_string(),
                    width: 1280,
                    height: 720,
                    fps: 30,
                    bitrate: 1_200_000,
                    filesize: Some(27_000_000),
                    hdr: false,
                    is_mp4: true,
                    codec: SimpleVideoCodec::H264,
                },
                RawVideoCandidate {
                    itag: 401,
                    url: "https://cdn.example/2160_av1_only.mp4".to_string(),
                    width: 3840,
                    height: 2160,
                    fps: 60,
                    bitrate: 12_000_000,
                    filesize: None,
                    hdr: false,
                    is_mp4: true,
                    codec: SimpleVideoCodec::Av1,
                },
            ],
            audio: vec![
                RawAudioCandidate {
                    itag: 140,
                    url: "https://cdn.example/128k.m4a".to_string(),
                    bitrate: 128_000,
                    sample_rate: 44_100,
                    filesize: Some(2_880_000),
                    is_m4a: true,
                    codec: SimpleAudioCodec::Aac,
                },
                RawAudioCandidate {
                    itag: 251,
                    url: "https://cdn.example/160k.webm".to_string(),
                    bitrate: 160_000,
                    sample_rate: 48_000,
                    filesize: Some(3_600_000),
                    is_m4a: false,
                    codec: SimpleAudioCodec::Opus,
                },
            ],
            hls_url: None,
            user_agent: "TestUA".to_string(),
            referer: String::new(),
            duration_secs: Some(180),
            title: Some("Rust & GTK4 Demo".to_string()),
            channel_id: String::new(),
            channel_name: String::new(),
            thumbnail_url: String::new(),
            view_count: None,
        };

        // 360p prefers single-connection progressive muxed MP4
        let p360 = select_direct_playback(&pool, Quality::P360).unwrap();
        assert_eq!(p360.primary_url, "https://cdn.example/360_muxed.mp4");
        assert!(p360.audio_url.is_none());
        assert_eq!(p360.title, "Rust & GTK4 Demo");

        // Verify quality-menu.lua JSON catalog & Kbps units
        let ytdl: Value = serde_json::from_str(p360.ytdl_json.as_deref().unwrap()).unwrap();
        assert_eq!(ytdl["title"], "Rust & GTK4 Demo");
        let fmts = ytdl["formats"].as_array().unwrap();
        // 2 audio + 3 deduplicated video heights (2160, 720, 360)
        assert_eq!(fmts.len(), 5);
        // At 720p, avc1 (v136) wins over av01 (v398)
        let f720 = fmts.iter().find(|f| f["height"] == 720).unwrap();
        assert_eq!(f720["format_id"], "v136");
        assert_eq!(f720["vcodec"], "avc1");
        assert_eq!(f720["resolution"], "1280x720");
        assert_eq!(f720["tbr"], 1200.0);
        // At 2160p, av01 is retained only because no H.264/VP9 exists at 2160p
        let f2160 = fmts.iter().find(|f| f["height"] == 2160).unwrap();
        assert_eq!(f2160["format_id"], "v401");
        // Active requested_formats highlights m18 + a140
        let req = ytdl["requested_formats"].as_array().unwrap();
        assert_eq!(req[0]["format_id"], "m18");
        assert_eq!(req[1]["format_id"], "a140");

        // 720p prefers H.264 over AV1 and pairs adaptive audio
        let p720 = select_direct_playback(&pool, Quality::P720).unwrap();
        assert_eq!(p720.primary_url, "https://cdn.example/720_h264.mp4");
        assert_eq!(
            p720.audio_url.as_deref(),
            Some("https://cdn.example/128k.m4a")
        );

        // Per-file ytdl-format override from quality-menu.lua (e.g. v136+a251)
        let entry = VerifiedPoolEntry {
            video_id: "vid1".to_string(),
            fetched_at: Instant::now(),
            pool: pool.clone(),
            hls_manifest: Some(
                "#EXTM3U\n#EXT-X-MEDIA:URI=\"https://manifest.example/a.m3u8\",TYPE=AUDIO,GROUP-ID=\"234\",DEFAULT=YES\n#EXT-X-STREAM-INF:BANDWIDTH=4200000,CODECS=\"mp4a.40.2,avc1.640028\",RESOLUTION=1920x1080,FRAME-RATE=60,AUDIO=\"234\"\nhttps://manifest.example/v1080.m3u8\n"
                    .to_string(),
            ),
        };
        let ovr_opus =
            resolve_from_verified_entry(&entry, Quality::P360, Some("v136+a251")).unwrap();
        assert_eq!(ovr_opus.primary_url, "https://cdn.example/720_h264.mp4");
        assert_eq!(
            ovr_opus.audio_url.as_deref(),
            Some("https://cdn.example/160k.webm")
        );
        let ovr_hls =
            resolve_from_verified_entry(&entry, Quality::P360, Some("hls-1080+a140")).unwrap();
        assert_eq!(ovr_hls.primary_url, "https://manifest.example/v1080.m3u8");
        assert_eq!(
            ovr_hls.audio_url.as_deref(),
            Some("https://manifest.example/a.m3u8")
        );
        let ovr_aud = resolve_from_verified_entry(&entry, Quality::P360, Some("a251")).unwrap();
        assert!(ovr_aud.is_audio_only);
        assert_eq!(ovr_aud.primary_url, "https://cdn.example/160k.webm");

        // AudioOnly picks direct audio stream
        let paud = select_direct_playback(&pool, Quality::AudioOnly).unwrap();
        assert!(paud.is_audio_only);
        assert_eq!(paud.primary_url, "https://cdn.example/128k.m4a");
        assert!(paud.audio_url.is_none());
    }

    #[test]
    fn test_cpn_and_hls_master_parsing() {
        assert_eq!(
            append_cpn_param("https://rr1.googlevideo.com/videoplayback?itag=18", "AbCdEf1234567890"),
            "https://rr1.googlevideo.com/videoplayback?itag=18&cpn=AbCdEf1234567890"
        );
        assert_eq!(
            append_cpn_param("https://rr1.googlevideo.com/videoplayback?itag=18&cpn=Existing", "New"),
            "https://rr1.googlevideo.com/videoplayback?itag=18&cpn=Existing"
        );

        let sample_m3u8 = "\
#EXTM3U
#EXT-X-MEDIA:URI=\"https://manifest.example/audio_en.m3u8\",TYPE=AUDIO,GROUP-ID=\"234\",NAME=\"English\",DEFAULT=YES,AUTOSELECT=YES
#EXT-X-STREAM-INF:BANDWIDTH=620000,CODECS=\"mp4a.40.2,avc1.4d401e\",RESOLUTION=640x360
https://manifest.example/v360_muxed.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2100000,CODECS=\"mp4a.40.2,avc1.4d401f\",RESOLUTION=1280x720,AUDIO=\"234\"
https://manifest.example/v720_video.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=4200000,CODECS=\"mp4a.40.2,avc1.640028\",RESOLUTION=1920x1080,AUDIO=\"234\"
https://manifest.example/v1080_video.m3u8
";

        let (u360, a360) = parse_hls_master_playlist(sample_m3u8, Quality::P360).unwrap();
        assert_eq!(u360, "https://manifest.example/v360_muxed.m3u8");
        assert!(a360.is_none());

        let (u720, a720) = parse_hls_master_playlist(sample_m3u8, Quality::P720).unwrap();
        assert_eq!(u720, "https://manifest.example/v720_video.m3u8");
        assert_eq!(
            a720.as_deref(),
            Some("https://manifest.example/audio_en.m3u8")
        );

        let (uaud, aaud) = parse_hls_master_playlist(sample_m3u8, Quality::AudioOnly).unwrap();
        assert_eq!(uaud, "https://manifest.example/audio_en.m3u8");
        assert!(aaud.is_none());
    }

    #[test]
    fn test_channel_shorts_lockup_and_header_stats_parsing() {
        let sample = serde_json::json!({
            "header": {
                "pageHeaderRenderer": {
                    "pageTitle": "Linus Tech Tips",
                    "content": {
                        "pageHeaderViewModel": {
                            "title": {
                                "dynamicTextViewModel": {
                                    "text": { "content": "Linus Tech Tips" }
                                }
                            },
                            "metadata": {
                                "contentMetadataViewModel": {
                                    "metadataRows": [
                                        {
                                            "metadataParts": [
                                                { "text": { "content": "@LinusTechTips" } }
                                            ]
                                        },
                                        {
                                            "metadataParts": [
                                                { "text": { "content": "16.2M subscribers" } },
                                                { "text": { "content": "7.9K videos" } }
                                            ]
                                        }
                                    ]
                                }
                            }
                        }
                    }
                }
            },
            "contents": {
                "twoColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "selected": true,
                                "endpoint": {
                                    "commandMetadata": {
                                        "webCommandMetadata": {
                                            "url": "/@LinusTechTips/shorts"
                                        }
                                    }
                                },
                                "content": {
                                    "richGridRenderer": {
                                        "contents": [
                                            {
                                                "richItemRenderer": {
                                                    "content": {
                                                        "shortsLockupViewModel": {
                                                            "entityId": "shorts-shelf-item-abc123XYZ89",
                                                            "onTap": {
                                                                "innertubeCommand": {
                                                                    "reelWatchEndpoint": {
                                                                        "videoId": "abc123XYZ89"
                                                                    }
                                                                }
                                                            },
                                                            "overlayMetadata": {
                                                                "primaryText": { "content": "Building a Tiny PC!" },
                                                                "secondaryText": { "content": "1.4M views" }
                                                            },
                                                            "thumbnailViewModel": {
                                                                "thumbnailViewModel": {
                                                                    "image": {
                                                                        "sources": [
                                                                            {
                                                                                "url": "https://i.ytimg.com/vi/abc123XYZ89/oar2.jpg",
                                                                                "width": 405,
                                                                                "height": 720
                                                                            }
                                                                        ]
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                            {
                                                "continuationItemRenderer": {
                                                    "continuationEndpoint": {
                                                        "continuationCommand": {
                                                            "token": "NEXT_SHORTS_TOKEN"
                                                        }
                                                    }
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let stats = extract_browse_header_stats(&sample);
        assert_eq!(stats.channel_name.as_deref(), Some("Linus Tech Tips"));
        assert_eq!(stats.handle.as_deref(), Some("@LinusTechTips"));
        assert_eq!(
            stats.subscriber_count_text.as_deref(),
            Some("16.2M subscribers")
        );
        assert_eq!(stats.video_count_text.as_deref(), Some("7.9K videos"));

        let fb = ChannelFallback {
            id: "UCXuqSBlHAE6Xw-yeJA0Tunw",
            name: "Linus Tech Tips",
            avatar_url: "",
        };
        let paged = parse_shorts_browse_response(&sample, "UCXuqSBlHAE6Xw-yeJA0Tunw", Some(&fb));
        assert_eq!(paged.items.len(), 1);
        assert_eq!(paged.items[0].id, "abc123XYZ89");
        assert_eq!(paged.items[0].title, "Building a Tiny PC!");
        assert_eq!(paged.items[0].view_count_text, "1.4M views");
        assert_eq!(paged.items[0].view_count, Some(1_400_000));
        assert!(paged.items[0].is_short);
        assert_eq!(
            paged.items[0].thumbnail_url,
            "https://i.ytimg.com/vi/abc123XYZ89/oar2.jpg"
        );
        assert_eq!(
            paged.next.as_ref().map(|c| c.ctoken.as_str()),
            Some("NEXT_SHORTS_TOKEN")
        );
        assert_eq!(
            paged.next.as_ref().map(|c| c.kind),
            Some(CursorKind::DirectChannelShorts)
        );
    }

    #[test]
    fn test_playlist_lockup_and_browse_parsing() {
        let lockup = serde_json::json!({
            "contentId": "PL1234567890ABCDEF",
            "contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST",
            "contentImage": {
                "collectionThumbnailViewModel": {
                    "primaryThumbnail": {
                        "thumbnailViewModel": {
                            "image": {
                                "sources": [
                                    { "url": "https://i.ytimg.com/vi/v1/mqdefault.jpg", "width": 320, "height": 180 }
                                ]
                            },
                            "overlays": [
                                {
                                    "thumbnailOverlayBadgeViewModel": {
                                        "thumbnailBadges": [
                                            { "thumbnailBadgeViewModel": { "text": "24 videos" } }
                                        ]
                                    }
                                }
                            ]
                        }
                    }
                }
            },
            "metadata": {
                "lockupMetadataViewModel": {
                    "title": { "content": "Rust Systems Programming" },
                    "metadata": {
                        "contentMetadataViewModel": {
                            "metadataRows": [
                                {
                                    "metadataParts": [
                                        { "text": { "content": "Jon Gjengset" } },
                                        { "text": { "content": "Playlist" } }
                                    ]
                                },
                                {}
                            ]
                        }
                    }
                }
            }
        });

        let pl_card = parse_playlist_lockup_json(&lockup, "").unwrap();
        assert_eq!(pl_card.id, "PL1234567890ABCDEF");
        assert_eq!(pl_card.name, "Rust Systems Programming");
        assert_eq!(pl_card.channel_name, "Jon Gjengset");
        assert_eq!(pl_card.video_count_text, "24 videos");

        let browse = serde_json::json!({
            "header": {
                "pageHeaderRenderer": {
                    "content": {
                        "pageHeaderViewModel": {
                            "title": {
                                "dynamicTextViewModel": {
                                    "text": { "content": "Rust Systems Programming" }
                                }
                            },
                            "metadata": {
                                "contentMetadataViewModel": {
                                    "metadataRows": [
                                        {
                                            "metadataParts": [
                                                {
                                                    "avatarStack": {
                                                        "avatarStackViewModel": {
                                                            "text": { "content": "by Jon Gjengset" }
                                                        }
                                                    }
                                                }
                                            ]
                                        },
                                        {
                                            "metadataParts": [
                                                { "text": { "content": "Playlist" } },
                                                { "text": { "content": "24 videos" } }
                                            ]
                                        }
                                    ]
                                }
                            }
                        }
                    }
                }
            },
            "contents": {
                "twoColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "selected": true,
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "itemSectionRenderer": {
                                                    "contents": [
                                                        {
                                                            "playlistVideoListRenderer": {
                                                                "contents": [
                                                                    {
                                                                        "playlistVideoRenderer": {
                                                                            "videoId": "vid_crust_01",
                                                                            "title": { "runs": [{ "text": "Crust of Rust: Lifetime Annotations" }] },
                                                                            "shortBylineText": { "runs": [{ "text": "Jon Gjengset" }] },
                                                                            "lengthSeconds": "5412",
                                                                            "lengthText": { "simpleText": "1:30:12" },
                                                                            "thumbnail": {
                                                                                "thumbnails": [
                                                                                    { "url": "https://i.ytimg.com/vi/vid_crust_01/mqdefault.jpg", "width": 320, "height": 180 }
                                                                                ]
                                                                            }
                                                                        }
                                                                    },
                                                                    {
                                                                        "continuationItemRenderer": {
                                                                            "continuationEndpoint": {
                                                                                "continuationCommand": {
                                                                                    "token": "PL_NEXT_PAGE"
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let remote = parse_playlist_browse_response(&browse, "PL1234567890ABCDEF").unwrap();
        assert_eq!(remote.name, "Rust Systems Programming");
        assert_eq!(remote.channel_name, "Jon Gjengset");
        assert_eq!(remote.video_count_text, "24 videos");
        assert_eq!(remote.videos.len(), 1);
        assert_eq!(remote.videos[0].id, "vid_crust_01");
        assert_eq!(remote.videos[0].duration_seconds, Some(5412));
        assert_eq!(
            remote.next.as_ref().map(|c| c.ctoken.as_str()),
            Some("PL_NEXT_PAGE")
        );
        assert_eq!(
            remote.next.as_ref().map(|c| c.kind),
            Some(CursorKind::DirectPlaylistVideos)
        );
    }

    #[test]
    fn test_podcast_lockup_playlist_and_synth_continuation() {
        let tok = build_initial_playlist_continuation_token("PL1234567890ABCDEF");
        assert!(!tok.is_empty());
        assert!(!tok.contains('+'));
        assert!(!tok.contains('/'));

        let podcast_browse = json!({
            "header": {
                "pageHeaderRenderer": {
                    "content": {
                        "pageHeaderViewModel": {
                            "title": {
                                "dynamicTextViewModel": {
                                    "text": { "content": "FarsaCast | The Uncomfortable Truth" }
                                }
                            },
                            "metadata": {
                                "contentMetadataViewModel": {
                                    "metadataRows": [
                                        {
                                            "metadataParts": [
                                                {
                                                    "avatarStack": {
                                                        "avatarStackViewModel": {
                                                            "text": { "content": "Ajeet Bharti" }
                                                        }
                                                    }
                                                }
                                            ]
                                        },
                                        {
                                            "metadataParts": [
                                                { "text": { "content": "Podcast" } },
                                                { "text": { "content": "13 videos" } }
                                            ]
                                        }
                                    ]
                                }
                            }
                        }
                    }
                }
            },
            "onResponseReceivedActions": [
                {
                    "appendContinuationItemsAction": {
                        "continuationItems": [
                            {
                                "lockupViewModel": {
                                    "contentId": "dQw4w9WgXcQ",
                                    "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
                                    "contentImage": {
                                        "thumbnailViewModel": {
                                            "image": {
                                                "sources": [
                                                    { "url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg", "width": 320, "height": 180 }
                                                ]
                                            },
                                            "overlays": [
                                                {
                                                    "thumbnailBottomOverlayViewModel": {
                                                        "badges": [
                                                            {
                                                                "thumbnailBadgeViewModel": {
                                                                    "text": "42:15"
                                                                }
                                                            }
                                                        ]
                                                    }
                                                }
                                            ]
                                        }
                                    },
                                    "metadata": {
                                        "lockupMetadataViewModel": {
                                            "title": { "content": "Episode 1 - The Beginning" },
                                            "metadata": {
                                                "contentMetadataViewModel": {
                                                    "metadataRows": [
                                                        {
                                                            "metadataParts": [
                                                                { "text": { "content": "120K views" } },
                                                                { "text": { "content": "2 weeks ago" } }
                                                            ]
                                                        }
                                                    ]
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            {
                                "continuationItemViewModel": {
                                    "continuationCommand": {
                                        "innertubeCommand": {
                                            "continuationCommand": {
                                                "token": "PODCAST_PAGE_2"
                                            }
                                        }
                                    }
                                }
                            }
                        ]
                    }
                }
            ]
        });

        let parsed = parse_playlist_browse_response(&podcast_browse, "PL_FARSACAST").unwrap();
        assert_eq!(parsed.name, "FarsaCast | The Uncomfortable Truth");
        assert_eq!(parsed.channel_name, "Ajeet Bharti");
        assert_eq!(parsed.video_count_text, "13 videos");
        assert_eq!(parsed.videos.len(), 1);
        assert_eq!(parsed.videos[0].id, "dQw4w9WgXcQ");
        assert_eq!(parsed.videos[0].title, "Episode 1 - The Beginning");
        assert_eq!(parsed.videos[0].duration_seconds, Some(2535));
        assert_eq!(
            parsed.next.as_ref().map(|c| c.ctoken.as_str()),
            Some("PODCAST_PAGE_2")
        );
    }

    #[test]
    fn test_shorts_search_params_and_parsing() {
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::Relevance,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                None
            ),
            "8AEB"
        );
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::Relevance,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                Some(3)
            ),
            "EgIQA_ABAQ%3D%3D"
        );
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::UploadDate,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                Some(3)
            ),
            "CAISAhAD8AEB"
        );
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::ViewCount,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                Some(3)
            ),
            "CAMSAhAD8AEB"
        );
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::Relevance,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                Some(9)
            ),
            "EgIQCfABAQ%3D%3D"
        );
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::UploadDate,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                Some(9)
            ),
            "CAISAhAJ8AEB"
        );
        assert_eq!(
            encode_direct_search_params(
                SearchSortOrder::ViewCount,
                UploadDateFilter::AnyTime,
                DurationFilter::AnyLength,
                Some(9)
            ),
            "CAMSAhAJ8AEB"
        );

        let search_payload = json!({
            "header": {
                "searchHeaderRenderer": {
                    "chipBar": {
                        "chipCloudRenderer": {
                            "chips": [
                                {
                                    "chipCloudChipRenderer": {
                                        "text": { "simpleText": "Shorts" },
                                        "navigationEndpoint": {
                                            "continuationCommand": { "token": "SHORTS_CHIP_CTOK" }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                }
            },
            "contents": {
                "twoColumnSearchResultsRenderer": {
                    "primaryContents": {
                        "sectionListRenderer": {
                            "contents": [
                                {
                                    "itemSectionRenderer": {
                                        "contents": [
                                            {
                                                "gridShelfViewModel": {
                                                    "contents": [
                                                        {
                                                            "shortsLockupViewModel": {
                                                                "entityId": "shorts-shelf-item-shortVid001",
                                                                "overlayMetadata": {
                                                                    "primaryText": { "content": "Rust GTK4 Tip #1" },
                                                                    "secondaryText": { "content": "84K views" }
                                                                },
                                                                "thumbnailViewModel": {
                                                                    "image": {
                                                                        "sources": [
                                                                            { "url": "https://i.ytimg.com/vi/shortVid001/oar2.jpg", "width": 405, "height": 720 }
                                                                        ]
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            },
                                            {
                                                "reelShelfRenderer": {
                                                    "items": [
                                                        {
                                                            "reelItemRenderer": {
                                                                "videoId": "shortVid002",
                                                                "headline": { "simpleText": "Quick Linux Trick" },
                                                                "viewCountText": { "simpleText": "120K views" },
                                                                "thumbnail": {
                                                                    "thumbnails": [
                                                                        { "url": "https://i.ytimg.com/vi/shortVid002/hq720_2.jpg", "width": 405, "height": 720 }
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            },
                                            {
                                                "videoRenderer": {
                                                    "videoId": "longVid9999",
                                                    "title": { "runs": [{ "text": "Full 45 Minute Deep Dive" }] },
                                                    "lengthText": { "simpleText": "45:00" },
                                                    "ownerText": { "runs": [{ "text": "Rust Channel" }] }
                                                }
                                            },
                                            {
                                                "videoRenderer": {
                                                    "videoId": "shortVid003",
                                                    "title": { "runs": [{ "text": "Direct Short Renderer" }] },
                                                    "lengthText": { "simpleText": "0:42" },
                                                    "navigationEndpoint": {
                                                        "reelWatchEndpoint": {
                                                            "videoId": "shortVid003",
                                                            "thumbnail": {
                                                                "thumbnails": [
                                                                    { "url": "https://i.ytimg.com/vi/shortVid003/oar2.jpg", "width": 405, "height": 720 }
                                                                ]
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        ]
                                    }
                                },
                                {
                                    "continuationItemRenderer": {
                                        "continuationEndpoint": {
                                            "continuationCommand": {
                                                "token": "NEXT_SHORTS_PAGE"
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        });

        assert_eq!(
            extract_search_header_shorts_chip(&search_payload),
            Some((Some("SHORTS_CHIP_CTOK".to_string()), None))
        );

        let all_res = parse_direct_search_response(&search_payload, DirectSearchMode::All);
        assert_eq!(all_res.items.len(), 4);
        assert_eq!(
            all_res.next.as_ref().map(|c| c.ctoken.as_str()),
            Some("NEXT_SHORTS_PAGE")
        );

        let shorts_only =
            parse_direct_search_response(&search_payload, DirectSearchMode::ShortsOnly);
        assert_eq!(shorts_only.items.len(), 3);
        let ids: Vec<&str> = shorts_only
            .items
            .iter()
            .filter_map(|it| match it {
                SearchItem::Video(v) => {
                    assert!(v.is_short);
                    Some(v.id.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["shortVid001", "shortVid002", "shortVid003"]);

        let synth_vd = synthesize_visitor_data();
        assert!(
            synth_vd.starts_with("Cg") && synth_vd.len() > 23,
            "unexpected visitor_data format: {synth_vd}"
        );
    }

    #[test]
    fn test_channel_sort_chip_token_extraction() {
        let response = json!({
            "header": {
                "chipCloudRenderer": {
                    "chips": [
                        { "chipCloudChipRenderer": {
                            "text": { "simpleText": "Latest" },
                            "navigationEndpoint": { "continuationCommand": { "token": "LATEST" } }
                        }},
                        { "chipCloudChipRenderer": {
                            "text": { "simpleText": "Popular" },
                            "navigationEndpoint": { "continuationCommand": { "token": "POPULAR" } }
                        }},
                        { "chipCloudChipRenderer": {
                            "text": { "simpleText": "Most popular" },
                            "navigationEndpoint": { "continuationCommand": { "token": "MOST_POPULAR" } }
                        }},
                        { "chipCloudChipRenderer": {
                            "text": { "simpleText": "Oldest" },
                            "navigationEndpoint": { "continuationCommand": { "token": "OLDEST" } }
                        }}
                    ]
                }
            }
        });
        assert_eq!(
            extract_channel_sort_chip_token(&response, "oldest"),
            Some("OLDEST".into())
        );
        assert_eq!(
            extract_channel_sort_chip_token(&response, "popular"),
            Some("POPULAR".into())
        );
        assert_eq!(
            extract_channel_sort_chip_token(&response, "most popular"),
            Some("MOST_POPULAR".into())
        );

        let sort_filter = json!({
            "sortFilterSubMenuRenderer": {
                "subMenuItems": [{
                    "title": "Oldest",
                    "serviceEndpoint": {
                        "innertubeCommand": {
                            "continuationCommand": { "token": "OLD_SORT_FILTER" }
                        }
                    }
                }]
            }
        });
        assert_eq!(
            extract_channel_sort_chip_token(&sort_filter, "oldest"),
            Some("OLD_SORT_FILTER".into())
        );

        let service_endpoint = json!({
            "chipCloudChipRenderer": {
                "text": { "simpleText": "Oldest" },
                "serviceEndpoint": {
                    "innertubeCommand": {
                        "continuationCommand": { "token": "OLDEST_SERVICE" }
                    }
                }
            }
        });
        assert_eq!(
            extract_channel_sort_chip_token(&service_endpoint, "oldest"),
            Some("OLDEST_SERVICE".into())
        );
    }
}
