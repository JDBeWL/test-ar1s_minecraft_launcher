use serde::{Deserialize, Serialize};
use std::fs;
use md5::Md5;
use digest::Digest;
use uuid::Uuid;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Listener};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinError;

#[derive(Error, Debug)]
pub enum LauncherError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),
    #[error("Custom error: {0}")]
    Custom(String),
}

impl serde::Serialize for LauncherError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("LauncherError", 1)?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftVersion {
    id: String,
    #[serde(rename = "type")]
    version_type: String,
    url: String,
    #[serde(default)]
    time: Option<String>,
    #[serde(rename(deserialize = "releaseTime", serialize = "release_time"))]
    release_time: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VersionManifest {
    latest: LatestVersions,
    versions: Vec<MinecraftVersion>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LatestVersions {
    release: String,
    snapshot: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LaunchOptions {
    version: String,
    game_dir: Option<String>,
    memory: Option<u32>,
    username: String,
    offline: bool,
    java_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GameConfig {
    pub game_dir: String,
    #[serde(default = "default_true")]
    pub version_isolation: bool,
    pub java_path: Option<String>,
    #[serde(default = "default_download_threads")]
    pub download_threads: u8,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_true")]
    pub isolate_saves: bool,
    #[serde(default = "default_true")]
    pub isolate_resourcepacks: bool,
    #[serde(default = "default_true")]
    pub isolate_logs: bool,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
}

fn default_download_threads() -> u8 {
    8
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
    Downloading,
    Completed,
    Cancelled,
    Error(String),
}

#[derive(Debug, Serialize, Clone)]
pub struct DownloadProgress {
    progress: u64,
    total: u64,
    speed: f64,
    status: DownloadStatus,
}

impl From<JoinError> for LauncherError {
    fn from(err: JoinError) -> Self {
        LauncherError::Custom(format!("Task join error: {}", err))
    }
}

#[derive(Debug, Serialize)]
pub struct GameDirInfo {
    path: String,
    versions: Vec<String>,
    total_size: u64,
}

// 获取 Minecraft 版本列表
#[tauri::command]
async fn get_versions() -> Result<VersionManifest, LauncherError> {
    // 创建带超时的HTTP客户端
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    
    // 优先使用国内镜像站
    let urls = [
        "https://bmclapi2.bangbang93.com/mc/game/version_manifest.json",
        "https://launchermeta.mojang.com/mc/game/version_manifest.json"
    ];

    // 创建日志目录
    let log_dir = PathBuf::from("logs");
    if !log_dir.exists() {
        fs::create_dir(&log_dir)?;
    }
    let log_file = log_dir.join("version_fetch.log");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)?;

    writeln!(log, "[{}] 开始获取版本列表", chrono::Local::now())?;

    for (i, url) in urls.iter().enumerate() {
        writeln!(log, "尝试第{}个源: {}", i+1, url)?;
        
        match fetch_versions(&client, url).await {
            Ok(manifest) => {
                writeln!(log, "成功获取版本列表，共{}个版本", manifest.versions.len())?;
                return Ok(manifest);
            },
            Err(e) => {
                writeln!(log, "获取失败: {}", e)?;
                continue;
            }
        }
    }

    Err(LauncherError::Custom("所有源都尝试失败，请检查网络连接".to_string()))
}

async fn fetch_versions(client: &reqwest::Client, url: &str) -> Result<VersionManifest, LauncherError> {
    let response = client.get(url).send().await?;
    let _content_type = response.headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string(); // 复制字符串以避免借用问题
    
    let bytes = response.bytes().await?;
        
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let text = text.trim_start_matches('\u{feff}').to_string();
    
    // 记录响应前100字符用于调试
    log::debug!("Received response (first 100 chars): {:?}", 
        text.chars().take(100).collect::<String>());
    
    // 记录原始响应
    let log_dir = PathBuf::from("logs");
    if !log_dir.exists() {
        fs::create_dir(&log_dir)?;
    }
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("version_fetch.log"))?;
    writeln!(log, "Raw response from {}:\n{}", url, text)?;

    // 解析JSON
    let manifest = serde_json::from_str::<VersionManifest>(&text)
        .map_err(|e| {
            writeln!(log, "JSON parse error: {}", e).ok();
            LauncherError::Json(e)
        })?;
    
    writeln!(log, "Parsed manifest with {} versions", manifest.versions.len())?;
    Ok(manifest)
}

// A struct to hold file download information
#[derive(Clone)]
struct DownloadJob {
    url: String,
    fallback_url: Option<String>,
    path: PathBuf,
    size: u64,
}

// 下载 Minecraft 版本
#[tauri::command]
async fn download_version(
    version_id: String,
    mirror: Option<String>,
    window: tauri::Window,
) -> Result<(), LauncherError> {
    let is_mirror = mirror.as_deref() == Some("bmcl");
    let base_url = if is_mirror {
        "https://bmclapi2.bangbang93.com"
    } else {
        "https://launchermeta.mojang.com"
    };

    // --- 1. 获取配置和路径 ---
    let config = load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let version_dir = game_dir.join("versions").join(&version_id);
    let (libraries_base_dir, assets_base_dir) = (
        game_dir.join("libraries"), 
        game_dir.join("assets")
    );

    // --- 2. 获取版本元数据 ---
    let client = reqwest::Client::new();
    let manifest: VersionManifest = client
        .get(&format!("{}/mc/game/version_manifest.json", base_url))
        .send().await?.json().await?;

    let version = manifest.versions.iter()
        .find(|v| v.id == version_id)
        .ok_or_else(|| LauncherError::Custom(format!("版本 {} 不存在", version_id)))?;

    let version_json_url = if is_mirror {
        version.url.replace("https://launchermeta.mojang.com", base_url)
    } else {
        version.url.clone()
    };
    
    let text = client.get(&version_json_url).send().await?.text().await?;
    let version_json: serde_json::Value = serde_json::from_str(&text)
        .or_else(|_| serde_json::from_str(text.trim_start_matches('\u{feff}')))
        .map_err(|_| LauncherError::Custom(format!("无法解析版本JSON for {}", version_id)))?;

    // --- 3. 收集所有待下载的文件 ---
    let mut downloads = Vec::new();

    // c. 客户端 JAR
    let client_info = &version_json["downloads"]["client"];
    let client_url = client_info["url"].as_str().ok_or_else(|| LauncherError::Custom("无法获取客户端下载URL".to_string()))?;
    let client_size = client_info["size"].as_u64().unwrap_or(0);
    let client_jar_path = version_dir.join(format!("{}.jar", version_id));
    downloads.push(DownloadJob {
        url: if is_mirror {
            client_url.replace("https://launcher.mojang.com", base_url)
                      .replace("https://piston-data.mojang.com", base_url)
        } else {
            client_url.to_string()
        },
        fallback_url: if is_mirror { Some(client_url.to_string()) } else { None },
        path: client_jar_path,
        size: client_size,
    });

    // --- b. 资源文件 (Assets) ---
    let assets_index_id = version_json["assetIndex"]["id"].as_str().ok_or_else(|| LauncherError::Custom("无法获取资源索引ID".to_string()))?;
    let assets_index_url = version_json["assetIndex"]["url"].as_str().ok_or_else(|| LauncherError::Custom("无法获取资源索引URL".to_string()))?;
    let assets_index_url = if is_mirror {
        assets_index_url.replace("https://launchermeta.mojang.com", base_url)
    } else {
        assets_index_url.to_string()
    };

    let assets_index_path = assets_base_dir.join("indexes").join(format!("{}.json", assets_index_id));
    fs::create_dir_all(assets_index_path.parent().unwrap())?;

    // Download asset index file if not exists or force download
    if !assets_index_path.exists() {
        let response = client.get(&assets_index_url).send().await?;
        let bytes = response.bytes().await?;
        fs::write(&assets_index_path, &bytes)?;
    }

    let index_content = fs::read_to_string(&assets_index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_content)?;

    if let Some(objects) = index["objects"].as_object() {
        for (_path, obj) in objects {
            let hash = obj["hash"].as_str().ok_or_else(|| LauncherError::Custom("资源缺少hash".to_string()))?;
            let size = obj["size"].as_u64().unwrap_or(0);
            let url = format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash);
            let file_path = assets_base_dir.join("objects").join(&hash[..2]).join(hash);
            downloads.push(DownloadJob { url, fallback_url: None, path: file_path, size });
        }
    }

    // c. 库文件 (Libraries)
    fs::create_dir_all(&libraries_base_dir)?;
    if let Some(libraries) = version_json["libraries"].as_array() {
        for lib in libraries {
            // 检查库是否适用于当前平台
            let mut should_download = true;
            if let Some(rules) = lib.get("rules").and_then(|r| r.as_array()) {
                should_download = false; // 默认不下载，除非规则明确允许
                for rule in rules {
                    let action = rule["action"].as_str().unwrap_or("");
                    if let Some(os) = rule.get("os") {
                        if let Some(name) = os["name"].as_str() {
                            let current_os = std::env::consts::OS;
                            if name == current_os {
                                should_download = action == "allow";
                            }
                        }
                    } else {
                        // 没有OS限制的规则适用于所有系统
                        should_download = action == "allow";
                    }
                }
            }
            
            // 特殊处理：如果是LWJGL库的natives，即使规则不允许也要下载
            let is_lwjgl = lib["name"].as_str().map_or(false, |name| name.contains("lwjgl"));
            let has_natives = lib.get("natives").is_some();
            
            // 对于LWJGL库的natives，忽略规则限制
            if is_lwjgl && has_natives {
                println!("DEBUG: 特殊处理LWJGL库: {}", lib["name"].as_str().unwrap_or("unknown"));
                should_download = true;
            }
            
            if !should_download {
                continue;
            }

            if let Some(artifact) = lib.get("downloads").and_then(|d| d.get("artifact")) {
                let url = artifact["url"].as_str().ok_or_else(|| LauncherError::Custom("库文件缺少URL".to_string()))?;
                let path = artifact["path"].as_str().ok_or_else(|| LauncherError::Custom("库文件缺少路径".to_string()))?;
                let size = artifact["size"].as_u64().unwrap_or(0);
                let download_url = if is_mirror {
                    url.replace("https://libraries.minecraft.net", base_url)
                } else {
                    url.to_string()
                };
                let file_path = libraries_base_dir.join(path);
                downloads.push(DownloadJob {
                    url: download_url,
                    fallback_url: if is_mirror { Some(url.to_string()) } else { None },
                    path: file_path,
                    size
                });
            }
            // 处理各种格式的natives库
            if let Some(natives) = lib.get("natives") {
                // 特殊处理LWJGL库：检查所有操作系统的natives，不仅仅是当前系统
                let is_lwjgl = lib["name"].as_str().map_or(false, |name| name.contains("lwjgl"));
                
                // 获取所有操作系统的classifier
                for (os_name, classifier_value) in natives.as_object().unwrap() {
                    let os_classifier = classifier_value.as_str().unwrap();
                    
                    // 对于LWJGL库，我们需要下载所有系统的natives
                    // 对于其他库，只下载当前系统的natives
                    if os_name == std::env::consts::OS || is_lwjgl {
                        println!("DEBUG: 处理natives库 - OS: {}, Classifier: {}, 是否LWJGL: {}", 
                                os_name, os_classifier, is_lwjgl);
                        
                        // 1. 首先尝试新版本格式：downloads.classifiers
                        if let Some(classifiers) = lib.get("downloads").and_then(|d| d.get("classifiers")) {
                            if let Some(artifact) = classifiers.get(os_classifier) {
                                if let (Some(url), Some(path)) = (artifact["url"].as_str(), artifact["path"].as_str()) {
                                    let size = artifact["size"].as_u64().unwrap_or(0);
                                    let download_url = if is_mirror {
                                        url.replace("https://libraries.minecraft.net", base_url)
                                    } else {
                                        url.to_string()
                                    };
                                    let file_path = libraries_base_dir.join(path);
                                    println!("DEBUG: 添加natives库(新格式): {} -> {}", url, file_path.display());
                                    downloads.push(DownloadJob {
                                        url: download_url,
                                        fallback_url: if is_mirror { Some(url.to_string()) } else { None },
                                        path: file_path,
                                        size,
                                    });
                                    continue; // 已处理，跳过后续逻辑
                                }
                            }
                        }
                        
                        // 2. 尝试直接从classifiers获取（某些版本的格式）
                        if let Some(classifiers) = lib.get("classifiers") {
                            if let Some(artifact) = classifiers.get(os_classifier) {
                                if let (Some(url), Some(path)) = (artifact["url"].as_str(), artifact["path"].as_str()) {
                                    let size = artifact["size"].as_u64().unwrap_or(0);
                                    let download_url = if is_mirror {
                                        url.replace("https://libraries.minecraft.net", base_url)
                                    } else {
                                        url.to_string()
                                    };
                                    let file_path = libraries_base_dir.join(path);
                                    println!("DEBUG: 添加natives库(直接classifiers): {} -> {}", url, file_path.display());
                                    downloads.push(DownloadJob {
                                        url: download_url,
                                        fallback_url: if is_mirror { Some(url.to_string()) } else { None },
                                        path: file_path,
                                        size,
                                    });
                                    continue; // 已处理，跳过后续逻辑
                                }
                            }
                        }
                        
                        // 3. 最后尝试通过名称构建路径（旧版本格式）
                        let name = lib["name"].as_str().unwrap_or("");
                        let parts: Vec<&str> = name.split(":").collect();
                        
                        if parts.len() >= 3 {
                            let group_id = parts[0].replace(".", "/");
                            let artifact_id = parts[1];
                            let version = parts[2];
                            
                            // 替换可能的变量
                            let classifier = os_classifier.replace("${arch}", if cfg!(target_pointer_width = "64") { "64" } else { "32" });
                            
                            // 构建natives路径
                            let natives_path = if artifact_id == "lwjgl" {
                                // LWJGL特殊处理 - 主库
                                format!("{}/{}-platform/{}/{}-platform-{}-{}.jar", 
                                       group_id, artifact_id, version, artifact_id, version, classifier)
                            } else if artifact_id == "lwjgl-platform" {
                                // LWJGL平台库
                                format!("{}/{}/{}/{}-{}-{}.jar", 
                                       group_id, artifact_id, version, artifact_id, version, classifier)
                            } else {
                                // 一般natives库
                                format!("{}/{}/{}/{}-{}-{}.jar", 
                                       group_id, artifact_id, version, artifact_id, version, classifier)
                            };
                            
                            // 构建下载URL
                            let natives_url = format!("https://libraries.minecraft.net/{}", natives_path);
                            let download_url = if is_mirror {
                                natives_url.replace("https://libraries.minecraft.net", base_url)
                            } else {
                                natives_url.clone()
                            };
                            
                            println!("DEBUG: 添加natives库(通过名称构建): {} -> {}", name, natives_path);
                            
                            // 添加到下载队列
                            let file_path = libraries_base_dir.join(&natives_path);
                            downloads.push(DownloadJob {
                                url: download_url,
                                fallback_url: if is_mirror { Some(natives_url) } else { None },
                                path: file_path,
                                size: 0, // 无法预知大小，下载后会验证
                            });
                        }
                    }
                }
            }
        }
    }

    // --- 4. 执行批量下载 ---
    download_all_files(downloads, &window).await?;

    // --- 5. 保存版本元数据文件 ---
    let version_json_path = version_dir.join(format!("{}.json", version_id));
    fs::write(version_json_path, text)?;

    Ok(())
}


async fn download_all_files(
    jobs: Vec<DownloadJob>,
    window: &tauri::Window,
) -> Result<(), LauncherError> {
    let config = load_config()?;
    let threads = config.download_threads as usize;
    let total_files = jobs.len() as u64;

    // --- Shared State ---
    let files_downloaded = Arc::new(AtomicU64::new(0));
    let bytes_downloaded = Arc::new(AtomicU64::new(0));
    let bytes_since_last = Arc::new(AtomicU64::new(0));
    let state = Arc::new(AtomicBool::new(true)); // true = running, false = cancelled/stopped
    let was_cancelled = Arc::new(AtomicBool::new(false));
    let error_occurred = Arc::new(tokio::sync::Mutex::new(None::<String>));

    // --- Cancellation Listener ---
    let state_clone = state.clone();
    let was_cancelled_clone = was_cancelled.clone();
    window.once("cancel-download", move |_| {
        state_clone.store(false, Ordering::SeqCst);
        was_cancelled_clone.store(true, Ordering::SeqCst);
    });

    // --- Progress Reporter Task ---
    let reporter_handle = {
        let files_downloaded = files_downloaded.clone();
        let bytes_since_last = bytes_since_last.clone();
        let state = state.clone();
        let window = window.clone();
        let report_interval = Duration::from_millis(500);

        tokio::spawn(async move {
            while state.load(Ordering::SeqCst) {
                tokio::time::sleep(report_interval).await;
                if !state.load(Ordering::SeqCst) { break; }

                let downloaded_count = files_downloaded.load(Ordering::SeqCst);
                let bytes_since = bytes_since_last.swap(0, Ordering::SeqCst);
                let speed = (bytes_since as f64 / 1024.0) / report_interval.as_secs_f64();

                let progress = DownloadProgress {
                    progress: downloaded_count,
                    total: total_files,
                    speed,
                    status: DownloadStatus::Downloading,
                };
                let _ = window.emit("download-progress", &progress);
            }
        })
    };

    // --- Download Worker Tasks ---
    let semaphore = Arc::new(tokio::sync::Semaphore::new(threads));
    let mut handles = vec![];

    for job in jobs {
        if !state.load(Ordering::SeqCst) { break; }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let state = state.clone();
        let files_downloaded = files_downloaded.clone();
        let bytes_downloaded = bytes_downloaded.clone();
        let bytes_since_last = bytes_since_last.clone();
        let error_occurred = error_occurred.clone();

        handles.push(tokio::spawn(async move {
            let mut current_job_error: Option<LauncherError> = None;
            const MAX_JOB_RETRIES: usize = 3;

            // Skip download if file exists and size matches
            if job.path.exists() {
                if let Ok(metadata) = fs::metadata(&job.path) {
                    if metadata.len() == job.size {
                        println!("DEBUG: 文件已存在且大小匹配，跳过下载: {}", job.path.display());
                        files_downloaded.fetch_add(1, Ordering::SeqCst);
                        bytes_downloaded.fetch_add(job.size, Ordering::SeqCst);
                        drop(permit);
                        return Ok(());
                    }
                }
            }

            for retry in 0..MAX_JOB_RETRIES {
                if !state.load(Ordering::SeqCst) { break; }
                println!("DEBUG: 尝试下载文件: {} (重试 {}/{})", job.url, retry + 1, MAX_JOB_RETRIES);
                match download_file(&job, &state, &bytes_downloaded, &bytes_since_last).await {
                    Ok(_) => {
                        files_downloaded.fetch_add(1, Ordering::SeqCst);
                        current_job_error = None;
                        break; // Success, no more retries needed
                    }
                    Err(e) => {
                        println!("ERROR: 文件下载失败: {} (重试 {}/{}) - {}", job.url, retry + 1, MAX_JOB_RETRIES, e);
                        current_job_error = Some(e);
                        tokio::time::sleep(Duration::from_secs(1 << retry)).await; // Exponential backoff
                    }
                }
            }

            if let Some(e) = current_job_error {
                state.store(false, Ordering::SeqCst); // Cancel all other downloads
                let mut error_guard = error_occurred.lock().await;
                if error_guard.is_none() {
                    *error_guard = Some(e.to_string());
                }
            }
            drop(permit);
            Ok::<(), LauncherError>(())
        }));
    }

    // --- Wait for all downloads to complete ---
    for handle in handles {
        let _ = handle.await;
    }

    // --- Finalize and Report Status ---
    state.store(false, Ordering::SeqCst);
    reporter_handle.await?;

    if was_cancelled.load(Ordering::SeqCst) {
        let _ = window.emit("download-progress", &DownloadProgress {
            progress: files_downloaded.load(Ordering::SeqCst),
            total: total_files,
            speed: 0.0,
            status: DownloadStatus::Cancelled,
        });
        return Err(LauncherError::Custom("下载已取消".to_string()));
    }

    if let Some(err_msg) = error_occurred.lock().await.take() {
        let _ = window.emit("download-progress", &DownloadProgress {
            progress: files_downloaded.load(Ordering::SeqCst),
            total: total_files,
            speed: 0.0,
            status: DownloadStatus::Error(err_msg.clone()),
        });
        return Err(LauncherError::Custom(err_msg));
    }
    
    // Final success event
    let _ = window.emit("download-progress", &DownloadProgress {
        progress: total_files,
        total: total_files,
        speed: 0.0,
        status: DownloadStatus::Completed,
    });

    Ok(())
}


async fn download_file(
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
) -> Result<(), LauncherError> {
    let client = reqwest::Client::new();

    // Try primary URL first
    match download_chunk(&client, &job.url, job, state, bytes_downloaded, bytes_since_last).await {
        Ok(_) => return Ok(()), // Success with primary URL
        Err(e) => {
            // If primary failed with 404 and fallback exists, try fallback
            if let Some(fallback_url) = &job.fallback_url {
                if let LauncherError::Http(err) = &e {
                    if err.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                        println!("DEBUG: Primary URL {} 404, trying fallback: {}", job.url, fallback_url);
                        match download_chunk(&client, fallback_url, job, state, bytes_downloaded, bytes_since_last).await {
                            Ok(_) => {
                                println!("DEBUG: Fallback download succeeded for {}", fallback_url);
                                return Ok(());
                            },
                            Err(fallback_e) => {
                                println!("ERROR: Fallback download failed for {}: {}", fallback_url, fallback_e);
                                return Err(fallback_e);
                            }
                        }
                    }
                }
            }
            // If primary failed for other reasons, or fallback not applicable/failed, return primary error
            return Err(e);
        }
    }
}

async fn download_chunk(
    client: &reqwest::Client,
    url: &str, // The actual URL to download from
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
) -> Result<(), LauncherError> {
    // Create parent directory
    if let Some(parent) = job.path.parent() {
        fs::create_dir_all(parent)?;
    }

    let response = client.get(url).send().await;
    let mut response = match response {
        Ok(res) => res,
        Err(e) => {
            println!("ERROR: HTTP请求失败 for {}: {}", url, e);
            return Err(LauncherError::Http(e));
        }
    };

    let status = response.status();
    if !status.is_success() {
        println!("ERROR: HTTP状态码非成功 for {}: {}", url, status);
        return Err(LauncherError::Http(response.error_for_status().unwrap_err()));
    }

    let mut file = tokio::fs::File::create(&job.path).await?;

    while let Some(chunk) = response.chunk().await? {
        if !state.load(Ordering::SeqCst) {
            return Err(LauncherError::Custom("Download cancelled".to_string()));
        }

        file.write_all(&chunk).await?;
        let len = chunk.len() as u64;
        bytes_downloaded.fetch_add(len, Ordering::Relaxed);
        bytes_since_last.fetch_add(len, Ordering::Relaxed);
    }

    // Verify file size after download
    let actual_size = file.metadata().await?.len();
    if actual_size != job.size {
        return Err(LauncherError::Custom(format!(
            "File size mismatch for {}: expected {}, got {}",
            job.path.display(), job.size, actual_size
        )));
    }

    Ok(())
}


// 启动 Minecraft
#[tauri::command]
async fn launch_minecraft(options: LaunchOptions, window: tauri::Window) -> Result<(), LauncherError> {
    // 保存用户名和UUID到配置文件
    let uuid = generate_offline_uuid(&options.username);
    let mut config = load_config()?;
    config.username = Some(options.username.clone());
    config.uuid = Some(uuid.clone());
    save_config(&config)?;
    
    // 继续使用更新后的配置
    let game_dir = PathBuf::from(&config.game_dir);
    let version_dir = game_dir.join("versions").join(&options.version);
    let version_json_path = version_dir.join(format!("{}.json", &options.version));

    println!("DEBUG: 尝试启动版本: {}", options.version);
    println!("DEBUG: 游戏目录: {}", game_dir.display());
    println!("DEBUG: 版本目录: {}", version_dir.display());
    println!("DEBUG: 版本JSON路径: {}", version_json_path.display());

    if !version_json_path.exists() {
        println!("ERROR: 版本JSON文件不存在: {}", version_json_path.display());
        return Err(LauncherError::Custom(format!("版本 {} 的json文件不存在!", options.version)));
    }

    let version_json_str = fs::read_to_string(&version_json_path)?;
    let version_json: serde_json::Value = serde_json::from_str(&version_json_str)?;

    let (libraries_base_dir, assets_base_dir) = (
        game_dir.join("libraries"), 
        game_dir.join("assets")
    );
    println!("DEBUG: 库文件目录: {}", libraries_base_dir.display());
    println!("DEBUG: 资源文件目录: {}", assets_base_dir.display());

    // --- 1. 准备隔离目录 ---
    let natives_dir = version_dir.join("natives");
    
    // 创建隔离目录
    if config.version_isolation {
        let isolate_dirs = vec![
            ("saves", config.isolate_saves),
            ("resourcepacks", config.isolate_resourcepacks),
            ("logs", config.isolate_logs),
        ];
        
        for (dir_name, should_isolate) in isolate_dirs {
            let dir_path = version_dir.join(dir_name);
            if should_isolate && !dir_path.exists() {
                fs::create_dir_all(&dir_path)?;
            }
        }
        
        // 复制options.txt
        let options_src = game_dir.join("options.txt");
        let options_dst = version_dir.join("options.txt");
        if options_src.exists() && !options_dst.exists() {
            fs::copy(&options_src, &options_dst)?;
        }
    }
    println!("DEBUG: Natives目录: {}", natives_dir.display());
    if natives_dir.exists() {
        println!("DEBUG: 清理旧的Natives目录: {}", natives_dir.display());
        fs::remove_dir_all(&natives_dir)?;
    }
    fs::create_dir_all(&natives_dir)?;

    if let Some(libraries) = version_json["libraries"].as_array() {
        for lib in libraries {
            if let Some(natives) = lib.get("natives") {
                println!("DEBUG: 发现Natives库: {:?}", lib);
                if let Some(os_classifier) = natives.get(std::env::consts::OS) {
                    println!("DEBUG: 正在查找的OS分类器: {}", os_classifier.as_str().unwrap_or("N/A"));
                    if let Some(artifact) = lib.get("downloads").and_then(|d| d.get("classifiers")).and_then(|c| c.get(os_classifier.as_str().unwrap())) {
                        println!("DEBUG: Natives Artifact: {:?}", artifact);
                        let lib_path = libraries_base_dir.join(artifact["path"].as_str().unwrap());
                        println!("DEBUG: 尝试解压Natives库: {}", lib_path.display());
                        if !lib_path.exists() {
                            println!("ERROR: Natives库文件不存在: {}", lib_path.display());
                            return Err(LauncherError::Custom(format!("Natives库文件不存在: {}", lib_path.display())));
                        }
                        let file = fs::File::open(&lib_path)?;
                        let mut archive = zip::ZipArchive::new(file)?;

                        for i in 0..archive.len() {
                            let mut file = archive.by_index(i)?;
                            let outpath = natives_dir.join(file.name());

                            // 检查是否需要排除
                            if let Some(extract_rules) = lib.get("extract") {
                                if let Some(exclude) = extract_rules.get("exclude").and_then(|e| e.as_array()) {
                                    if exclude.iter().any(|v| file.name().starts_with(v.as_str().unwrap())) {
                                        continue;
                                    }
                                }
                            }

                            if (*file.name()).ends_with('/') {
                                fs::create_dir_all(&outpath)?;
                            } else {
                                if let Some(p) = outpath.parent() {
                                    if !p.exists() {
                                        fs::create_dir_all(&p)?;
                                    }
                                }
                                let mut outfile = fs::File::create(&outpath)?;
                                io::copy(&mut file, &mut outfile)?;
                                println!("DEBUG: 解压Natives文件: {}", outpath.display());
                            }
                        }
                    }
                }
            }
        }
    }

    // --- 2. 构建 Classpath ---
    let mut classpath = vec![];
    if let Some(libraries) = version_json["libraries"].as_array() {
        for lib in libraries {
            if lib.get("natives").is_some() { continue; } // 跳过Natives库

            if let Some(rules) = lib.get("rules").and_then(|r| r.as_array()) {
                let mut allowed = true;
                for rule in rules {
                    if let Some(os) = rule.get("os") {
                        if let Some(name) = os["name"].as_str() {
                            if name == std::env::consts::OS {
                                allowed = rule["action"].as_str() == Some("allow");
                            } else {
                                allowed = rule["action"].as_str() != Some("allow");
                            }
                        }
                    }
                }
                if !allowed { continue; }
            }

            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let lib_path = libraries_base_dir.join(path);
                println!("DEBUG: 添加到Classpath的库: {}", lib_path.display());
                if !lib_path.exists() {
                    println!("ERROR: Classpath中的库文件不存在: {}", lib_path.display());
                    return Err(LauncherError::Custom(format!("Classpath中的库文件不存在: {}", lib_path.display())));
                }
                classpath.push(lib_path);
            }
        }
    }
    let main_game_jar_path = version_dir.join(format!("{}.jar", &options.version));
    println!("DEBUG: 主游戏JAR路径: {}", main_game_jar_path.display());
    if !main_game_jar_path.exists() {
        println!("ERROR: 主游戏JAR文件不存在: {}", main_game_jar_path.display());
        return Err(LauncherError::Custom(format!("主游戏JAR文件不存在: {}", main_game_jar_path.display())));
    }
    classpath.push(main_game_jar_path);
    let classpath_str = classpath.iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>().join(if cfg!(windows) { ";" } else { ":" });
    println!("DEBUG: 最终Classpath: {}", classpath_str);

    // --- 3. 获取主类和参数 ---
    let main_class = version_json["mainClass"].as_str().ok_or_else(|| LauncherError::Custom("无法在json中找到mainClass".to_string()))?;
    let assets_index = version_json["assetIndex"]["id"].as_str().unwrap_or(&options.version);
    let assets_dir = assets_base_dir;

    // 替换通用占位符的辅助函数
    let replace_placeholders = |arg: &str| -> String {
        let actual_game_dir = if config.version_isolation {
            version_dir.to_string_lossy().to_string()
        } else {
            game_dir.to_string_lossy().to_string()
        };

        arg.replace("${auth_player_name}", &options.username)
           .replace("${version_name}", &options.version)
           .replace("${game_directory}", &actual_game_dir)
           .replace("${assets_root}", &assets_dir.to_string_lossy().to_string())
           .replace("${assets_index_name}", assets_index)
           .replace("${auth_uuid}", &generate_offline_uuid(&options.username))
           .replace("${auth_access_token}", "0")
           .replace("${user_type}", "legacy")
           .replace("${version_type}", version_json["type"].as_str().unwrap_or("release"))
           .replace("${user_properties}", "{}")
    };

    let mut jvm_args = vec![];
    let mut game_args_vec = vec![];

    // 处理新版 (1.13+) `arguments` 格式
    if let Some(arguments) = version_json.get("arguments") {
        if let Some(jvm) = arguments["jvm"].as_array() {
            for arg in jvm {
                if let Some(s) = arg.as_str() {
                    jvm_args.push(replace_placeholders(s));
                } else if let Some(obj) = arg.as_object() {
                    // 处理带规则的JVM参数
                    let mut allowed = true;
                    if let Some(rules) = obj.get("rules").and_then(|r| r.as_array()) {
                         for rule in rules {
                            if let Some(os) = rule.get("os") {
                                if let Some(name) = os["name"].as_str() {
                                    if name == std::env::consts::OS {
                                        allowed = rule["action"].as_str() == Some("allow");
                                    } else {
                                        allowed = rule["action"].as_str() != Some("allow");
                                    }
                                }
                            }
                        }
                    }
                    if allowed {
                        if let Some(value) = obj.get("value") {
                            if let Some(s) = value.as_str() {
                                jvm_args.push(replace_placeholders(s));
                            } else if let Some(arr) = value.as_array() {
                                for item in arr {
                                    jvm_args.push(replace_placeholders(item.as_str().unwrap()));
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(game) = arguments["game"].as_array() {
            for arg in game {
                if let Some(s) = arg.as_str() {
                    game_args_vec.push(replace_placeholders(s));
                }
            }
        }
    } 
    // 处理旧版 `minecraftArguments` 格式
    else if let Some(mc_args) = version_json["minecraftArguments"].as_str() {
        game_args_vec = mc_args.split(' ').map(replace_placeholders).collect();
    }

    // --- 4. 组装Java启动参数 ---
    let java_path = {
        // 1. 首先尝试使用配置中的Java路径
        if let Some(config_path) = load_config()?.java_path {
            if PathBuf::from(&config_path).exists() {
                config_path
            } else {
                // 2. 如果配置路径不存在，尝试在PATH中查找
                if Command::new("java").arg("-version").output().is_ok() {
                    "java".to_string()
                } else {
                    return Err(LauncherError::Custom(format!(
                        "配置的Java路径不存在且系统PATH中未找到Java: {}",
                        config_path
                    )));
                }
            }
        } else {
            // 3. 如果未配置路径，尝试在PATH中查找
            if Command::new("java").arg("-version").output().is_ok() {
                "java".to_string()
            } else {
                return Err(LauncherError::Custom(
                    "未配置Java路径且系统PATH中未找到Java".to_string(),
                ));
            }
        }
    };
    println!("DEBUG: 使用的Java路径: {}", java_path);

    let mut final_args = vec![
        format!("-Xmx{}M", options.memory.unwrap_or(2048)),
        format!("-Djava.library.path={}", natives_dir.to_string_lossy()),
    ];
    final_args.extend(jvm_args);
    final_args.push("-cp".to_string());
    final_args.push(classpath_str);
    final_args.push(main_class.to_string());
    final_args.extend(game_args_vec);

    // --- 5. 启动游戏 ---
    let mut command = Command::new(&java_path);
    command.args(&final_args);
    command.current_dir(&game_dir);
    
    // 在Windows上隐藏命令行窗口
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW = 0x08000000
        // 使用这个标志可以隐藏命令行窗口
        command.creation_flags(0x08000000);
    }

    println!("DEBUG: 最终启动命令: {:?}", command);
    window.emit("launch-command", format!("{:?}", command))?;

    // 启动游戏进程但不等待它结束
    let child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    
    println!("DEBUG: 游戏已启动，PID: {:?}", child.id());
    
    // 发送游戏启动成功的事件到前端
    window.emit("minecraft-launched", format!("游戏已启动，PID: {}", child.id()))?;
    
    // 在后台线程中监控游戏进程，不阻塞主线程
    let window_clone = window.clone();
    std::thread::spawn(move || {
        match child.wait_with_output() {
            Ok(output) => {
                let status = output.status;
                println!("DEBUG: 游戏进程退出，状态码: {:?}", status.code());
                // 发送游戏退出事件到前端
                let _ = window_clone.emit("minecraft-exited", format!("游戏已退出，状态码: {:?}", status.code()));
            }
            Err(e) => {
                println!("DEBUG: 等待游戏进程时出错: {:?}", e);
                // 发送错误事件到前端
                let _ = window_clone.emit("minecraft-error", format!("监控游戏进程时出错: {}", e));
            }
        }
    });

    println!("DEBUG: 游戏成功启动");
    Ok(())
}

// 获取游戏目录
#[tauri::command(rename = "get_config")]
async fn get_config() -> Result<GameConfig, LauncherError> {
    load_config()
}

#[tauri::command]
async fn load_config_key(key: String) -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    match key.as_str() {
        "javaPath" => Ok(config.java_path),
        "gameDir" => Ok(Some(config.game_dir)),
        "versionIsolation" => Ok(Some(config.version_isolation.to_string())),
        "downloadThreads" => Ok(Some(config.download_threads.to_string())),
        "language" => Ok(config.language),
        "isolateSaves" => Ok(Some(config.isolate_saves.to_string())),
        "isolateResourcepacks" => Ok(Some(config.isolate_resourcepacks.to_string())),
        "isolateLogs" => Ok(Some(config.isolate_logs.to_string())),
        "username" => Ok(config.username),
        "uuid" => Ok(config.uuid),
        _ => Err(LauncherError::Custom(format!("Unknown config key: {}", key))),
    }
}

#[tauri::command]
async fn save_config_key(key: String, value: String) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    match key.as_str() {
        "javaPath" => config.java_path = Some(value),
        "gameDir" => config.game_dir = value,
        "versionIsolation" => config.version_isolation = value.parse().map_err(|_| LauncherError::Custom("Invalid boolean value for versionIsolation".to_string()))?,
        "downloadThreads" => config.download_threads = value.parse().map_err(|_| LauncherError::Custom("Invalid u8 value for downloadThreads".to_string()))?,
        "language" => config.language = Some(value),
        "isolateSaves" => config.isolate_saves = value.parse().map_err(|_| LauncherError::Custom("Invalid boolean value for isolateSaves".to_string()))?,
        "isolateResourcepacks" => config.isolate_resourcepacks = value.parse().map_err(|_| LauncherError::Custom("Invalid boolean value for isolateResourcepacks".to_string()))?,
        "isolateLogs" => config.isolate_logs = value.parse().map_err(|_| LauncherError::Custom("Invalid boolean value for isolateLogs".to_string()))?,
        "username" => config.username = Some(value),
        "uuid" => config.uuid = Some(value),
        _ => return Err(LauncherError::Custom(format!("Unknown config key: {}", key))),
    }
    save_config(&config)?;
    Ok(())
}

#[tauri::command]
fn get_game_dir() -> Result<String, LauncherError> {
    let config = load_config()?;
    Ok(config.game_dir)
}

// 选择游戏目录
#[tauri::command]
async fn select_game_dir(_window: tauri::Window) -> Result<String, LauncherError> {
    // 现在由前端直接处理对话框选择
    get_game_dir()
}

// 获取游戏目录信息
#[tauri::command]
async fn get_game_dir_info() -> Result<GameDirInfo, LauncherError> {
    let game_dir_str = get_game_dir()?;
    let versions_dir = PathBuf::from(&game_dir_str).join("versions");
    let mut versions = Vec::new();

    if versions_dir.is_dir() {
        for entry in fs::read_dir(versions_dir)? {
            if let Ok(entry) = entry {
                if entry.file_type()?.is_dir() {
                    let version_id = entry.file_name().to_string_lossy().into_owned();
                    let version_json_path = entry.path().join(format!("{}.json", version_id));
                    if version_json_path.exists() {
                        versions.push(version_id);
                    }
                }
            }
        }
    }

    // total_size is not used in the frontend, so we can just return 0
    Ok(GameDirInfo {
        path: game_dir_str,
        versions,
        total_size: 0,
    })
}

// 设置游戏目录
#[tauri::command]
async fn set_game_dir(path: String, window: tauri::Window) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.game_dir = path.clone();
    save_config(&config)?;
    
    // 发送事件通知前端游戏目录已更改
    window.emit("game-dir-changed", &path)?;
    
    Ok(())
}

// 设置版本隔离
#[tauri::command]
async fn set_version_isolation(enabled: bool) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.version_isolation = enabled;
    save_config(&config)?;
    Ok(())
}

// 查找Java安装路径
#[tauri::command]
async fn find_java_installations_command() -> Result<Vec<String>, LauncherError> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
        let program_files_x86 =
            std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());

        // 检查常见Java安装路径
        let java_dirs = vec![
            format!("{}\\Java", program_files),
            format!("{}\\Java", program_files_x86),
            r"C:\Program Files\Java".to_string(),
            r"C:\Program Files (x86)\Java".to_string(),
        ];

        for dir in java_dirs {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let dir_name = entry.file_name().to_string_lossy().to_lowercase();
                        if dir_name.contains("jdk") || dir_name.contains("jre") {
                            let java_exe = entry.path().join("bin").join("java.exe");
                            if java_exe.exists() {
                                paths.push(java_exe.to_string_lossy().into_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    // 检查PATH中的java
    if Command::new("java").arg("-version").output().is_ok() {
        paths.push("java".to_string());
    }

    // 去重并排序
    paths.sort();
    paths.dedup();

    Ok(paths)
}

// 设置Java路径
#[tauri::command]
async fn set_java_path_command(path: String) -> Result<(), LauncherError> {
    // 标准化路径格式
    let normalized_path = if cfg!(windows) {
        path.replace("/", "\\") // 统一为Windows路径分隔符
    } else {
        path.replace("\\", "/") // 统一为Unix路径分隔符
    };

    // 验证路径是否有效
    if !PathBuf::from(&normalized_path).exists() {
        return Err(LauncherError::Custom(format!("Java路径不存在: {}", normalized_path)));
    }

    let mut config = load_config()?;
    config.java_path = Some(normalized_path);
    save_config(&config)?;
    Ok(())
}

// 辅助函数
fn load_config() -> Result<GameConfig, LauncherError> {
    let config_path = get_config_path()?;
    
    if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        let config: GameConfig = serde_json::from_str(&content)?;
        Ok(config)
    } else {
        // 获取可执行文件路径并确保其存在
        let exe_path = std::env::current_exe()?;
        let exe_dir = exe_path.parent()
            .ok_or_else(|| LauncherError::Custom("无法获取可执行文件目录".to_string()))?;
        
        // 创建路径变量并确保所有权
        let mc_dir = exe_dir.join(".minecraft");
        let mc_dir_str = mc_dir.to_string_lossy().into_owned();
        
        // 创建目录结构
        if !mc_dir.exists() {
            fs::create_dir_all(&mc_dir)?;
            // 创建必要的子目录
            let sub_dirs = ["versions", "libraries", "assets", "saves", "resourcepacks", "logs"];
            for dir in sub_dirs {
                fs::create_dir_all(mc_dir.join(dir))?;
            }
        }

        // 创建并返回配置
        let config = GameConfig {
            game_dir: mc_dir_str,
            version_isolation: true,
            java_path: None,
            download_threads: 8,
            language: Some("zh_cn".to_string()),
            isolate_saves: true,
            isolate_resourcepacks: true,
            isolate_logs: true,
            username: None,
            uuid: None,
        };

        // 保存配置
        save_config(&config)?;

        Ok(config)
    }
}

fn save_config(config: &GameConfig) -> Result<(), LauncherError> {
    let config_path = get_config_path()?;
    fs::write(config_path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

// 生成Minecraft离线模式UUID
fn generate_offline_uuid(username: &str) -> String {
    // 首先检查配置中是否已有保存的UUID
    if let Ok(config) = load_config() {
        // 如果用户名匹配且已有UUID，则直接返回保存的UUID
        if let (Some(saved_username), Some(saved_uuid)) = (&config.username, &config.uuid) {
            if saved_username == username {
                return saved_uuid.clone();
            }
        }
    }
    
    // 如果没有保存的UUID或用户名不匹配，则生成新的UUID
    // Minecraft官方算法: MD5("OfflinePlayer:" + username)
    let mut hasher = Md5::new();
    hasher.update(b"OfflinePlayer:");
    hasher.update(username.as_bytes());
    let result = hasher.finalize();
    
    // 将MD5哈希转换为UUID格式
    let bytes: [u8; 16] = result.into();
    let uuid = Uuid::new_v5(&Uuid::NAMESPACE_DNS, &bytes);
    
    uuid.to_string()
}

fn get_config_path() -> Result<PathBuf, LauncherError> {
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent()
        .ok_or_else(|| LauncherError::Custom("无法获取可执行文件目录".to_string()))?;
    
    Ok(exe_dir.join("ar1s.json"))
}


#[tauri::command]
fn get_download_threads() -> Result<u8, LauncherError> {
    let config = load_config()?;
    Ok(config.download_threads)
}

#[tauri::command]
async fn set_download_threads(threads: u8) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.download_threads = threads;
    save_config(&config)?;
    Ok(())
}

#[tauri::command]
async fn validate_java_path(path: String) -> Result<bool, LauncherError> {
    let java_exe = PathBuf::from(&path);
    if java_exe.is_file() {
        // Try to run "java -version" to confirm it's a valid Java executable
        let output = Command::new(&java_exe)
            .arg("-version")
            .output();

        match output {
            Ok(out) => {
                // Check if stderr contains "java version" or "openjdk version"
                let stderr_str = String::from_utf8_lossy(&out.stderr);
                Ok(out.status.success() && (stderr_str.contains("java version") || stderr_str.contains("openjdk version")))
            },
            Err(_) => Ok(false), // Command failed to execute
        }
    } else if path.to_lowercase() == "java" {
        // If path is just "java", check if it's in PATH
        let output = Command::new("java")
            .arg("-version")
            .output();
        match output {
            Ok(out) => {
                let stderr_str = String::from_utf8_lossy(&out.stderr);
                Ok(out.status.success() && (stderr_str.contains("java version") || stderr_str.contains("openjdk version")))
            },
            Err(_) => Ok(false),
        }
    }
    else {
        Ok(false)
    }
}

// 验证版本文件完整性
#[tauri::command]
async fn validate_version_files(version_id: String) -> Result<Vec<String>, LauncherError> {
    let config = load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let version_dir = game_dir.join("versions").join(&version_id);
    let version_json_path = version_dir.join(format!("{}.json", &version_id));

    let mut missing_files = Vec::new();

    // 1. 检查版本JSON文件
    if !version_json_path.exists() {
        missing_files.push(format!("版本JSON文件不存在: {}", version_json_path.display()));
        return Ok(missing_files); // 如果JSON不存在，后续检查也无法进行
    }

    let version_json_str = fs::read_to_string(&version_json_path)?;
    let version_json: serde_json::Value = serde_json::from_str(&version_json_str)?;

    let (libraries_base_dir, _assets_base_dir) = (
        game_dir.join("libraries"), 
        game_dir.join("assets")
    );

    // 2. 检查主游戏JAR文件
    let main_game_jar_path = version_dir.join(format!("{}.jar", &version_id));
    if !main_game_jar_path.exists() {
        missing_files.push(format!("主游戏JAR文件不存在: {}", main_game_jar_path.display()));
    }

    // 3. 检查所有库文件 (包括Natives)
    if let Some(libraries) = version_json["libraries"].as_array() {
        for lib in libraries {
            // 检查Natives库
            if let Some(natives) = lib.get("natives") {
                println!("DEBUG: validate_version_files: 发现Natives库: {:?}", lib);
                if let Some(os_classifier) = natives.get(std::env::consts::OS) {
                    println!("DEBUG: validate_version_files: 正在查找的OS分类器: {}", os_classifier.as_str().unwrap_or("N/A"));
                    if let Some(artifact) = lib.get("downloads").and_then(|d| d.get("classifiers")).and_then(|c| c.get(os_classifier.as_str().unwrap())) {
                        println!("DEBUG: validate_version_files: Natives Artifact: {:?}", artifact);
                        let lib_path = libraries_base_dir.join(artifact["path"].as_str().unwrap());
                        println!("DEBUG: validate_version_files: 检查Natives库路径: {}", lib_path.display());
                        if !lib_path.exists() {
                            missing_files.push(format!("Natives库文件不存在: {}", lib_path.display()));
                        }
                    }
                }
            } else { // 检查普通库
                if let Some(rules) = lib.get("rules").and_then(|r| r.as_array()) {
                    let mut allowed = true;
                    for rule in rules {
                        if let Some(os) = rule.get("os") {
                            if let Some(name) = os["name"].as_str() {
                                if name == std::env::consts::OS {
                                    allowed = rule["action"].as_str() == Some("allow");
                                } else {
                                    allowed = rule["action"].as_str() != Some("allow");
                                }
                            }
                        }
                    }
                    if !allowed { continue; }
                }

                if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                    let lib_path = libraries_base_dir.join(path);
                    println!("DEBUG: validate_version_files: 检查库路径: {}", lib_path.display());
                    if !lib_path.exists() {
                        missing_files.push(format!("库文件不存在: {}", lib_path.display()));
                    }
                }
            }
        }
    }

    Ok(missing_files)
}

// 获取保存的用户名
#[allow(dead_code)]
#[tauri::command]
async fn get_saved_username() -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    Ok(config.username)
}

// 设置保存的用户名
#[allow(dead_code)]
#[tauri::command]
async fn set_saved_username(username: String) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.username = Some(username);
    save_config(&config)?;
    Ok(())
}

// 获取保存的UUID
#[allow(dead_code)]
#[tauri::command]
async fn get_saved_uuid() -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    Ok(config.uuid)
}

// 设置保存的UUID
#[allow(dead_code)]
#[tauri::command]
async fn set_saved_uuid(uuid: String) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.uuid = Some(uuid);
    save_config(&config)?;
    Ok(())
}

// 尝试修改WebView2进程名称的函数
#[cfg(target_os = "windows")]
fn try_rename_webview_process() {
    use std::thread;
    use std::time::Duration;
    
    // 在后台线程中执行，以便不阻塞主线程
    thread::spawn(|| {
        // 等待一段时间，确保WebView2进程已经启动
        thread::sleep(Duration::from_secs(2));
        
        println!("尝试修改WebView2进程名称");
        
        // 注意：这里只是记录日志，实际上我们无法直接修改WebView2进程的名称
        // 因为WebView2进程是由Microsoft Edge WebView2运行时控制的
        println!("WebView2进程名称由Microsoft Edge WebView2运行时控制，无法直接修改");
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        // 对话框功能已内置
        .plugin(tauri_plugin_http::init())
        .setup(|app| {
            // 设置WebView进程名称
            #[cfg(target_os = "windows")]
            {
                // 尝试修改WebView2进程名称
                try_rename_webview_process();
                
                // 在setup中注册一个事件处理器，当窗口创建后执行
                app.listen("tauri://window-created", move |_| {
                    // 这里无法直接访问窗口，但我们可以在前端代码中设置用户代理
                    println!("窗口已创建，尝试设置WebView用户代理");
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_versions,
            download_version,
            launch_minecraft,
            get_config,
            get_game_dir,
            get_game_dir_info,
            set_game_dir,
            select_game_dir,
            set_version_isolation,
            find_java_installations_command,
            set_java_path_command,
            load_config_key,
            save_config_key,
            validate_java_path,
            get_download_threads,
            set_download_threads,
            validate_version_files,
            get_saved_username,
            set_saved_username,
            get_saved_uuid,
            set_saved_uuid
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}