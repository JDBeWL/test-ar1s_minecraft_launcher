use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// 默认下载线程数
pub fn default_download_threads() -> u8 {
    8
}

// 默认最大内存 (MB)
pub fn default_max_memory() -> u32 {
    4096
}

// 默认为true的辅助函数
pub fn default_true() -> bool {
    true
}

// 游戏配置
#[derive(Debug, Serialize, Deserialize)]
pub struct GameConfig {
    pub game_dir: String,
    #[serde(default = "default_true")]
    pub version_isolation: bool,
    pub java_path: Option<String>,
    #[serde(default = "default_download_threads")]
    pub download_threads: u8,
    pub language: Option<String>,
    #[serde(default = "default_true")]
    pub isolate_saves: bool,
    #[serde(default = "default_true")]
    pub isolate_resourcepacks: bool,
    #[serde(default = "default_true")]
    pub isolate_logs: bool,
    pub username: Option<String>,
    pub uuid: Option<String>,
    #[serde(default = "default_max_memory")]
    pub max_memory: u32,
}

// 游戏目录信息
#[derive(Debug, Serialize, Deserialize)]
pub struct GameDirInfo {
    pub path: String,
    pub versions: Vec<String>,
    pub total_size: u64,
}

// Minecraft版本
#[derive(Debug, Serialize, Deserialize)]
pub struct MinecraftVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    pub time: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
}

// 版本清单
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<MinecraftVersion>,
}

// 最新版本
#[derive(Debug, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

// 启动选项
#[derive(Debug, Serialize, Deserialize)]
pub struct LaunchOptions {
    pub version: String,
    pub username: String,
    pub memory: Option<u32>,
}

// 下载状态
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DownloadStatus {
    Downloading,
    Completed,
    Cancelled,
    Error(String),
}

// 下载进度
#[derive(Debug, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub progress: u64,
    pub total: u64,
    pub speed: f64,
    pub status: DownloadStatus,
}

// 下载任务
#[derive(Debug)]
#[derive(Clone)]
pub struct DownloadJob {
    pub url: String,
    pub fallback_url: Option<String>,
    pub path: PathBuf,
    pub size: u64,
}