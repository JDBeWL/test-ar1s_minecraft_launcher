use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tauri::Emitter;

use download::{download_all_files as download_all_files_impl};

mod error;
mod models;
mod launcher;
pub mod download;
pub mod auth;
pub mod java;

pub use error::LauncherError;
pub use models::*;

// 直接导出launcher模块中的函数
pub use launcher::launch_minecraft;

// 获取 Minecraft 版本列表
// 初始化日志系统
fn init_logging() -> Result<PathBuf, LauncherError> {
    println!("[DEBUG] 正在初始化日志系统..."); // 控制台输出
    
    let config = load_config()?;
    let minecraft_dir = PathBuf::from(&config.game_dir);
    let log_dir = minecraft_dir.join("logs");
    
    println!("[DEBUG] 日志目录路径: {}", log_dir.display());
    
    fs::create_dir_all(&log_dir)
        .map_err(|e| LauncherError::Custom(format!("无法创建日志目录: {}", e)))?;
        
    println!("[DEBUG] 日志目录创建成功");
    Ok(log_dir)
}

#[tauri::command]
async fn get_versions() -> Result<VersionManifest, LauncherError> {
    // 初始化日志系统
    let _ = init_logging()?;
    
    // 创建带超时的HTTP客户端 (30秒超时)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| {
            let log_file = PathBuf::from("logs").join("version_fetch.log");
            let mut log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file)
                .unwrap_or_else(|_| std::process::exit(1));
            let _ = writeln!(log, "[ERROR] 创建HTTP客户端失败: {}", e);
            LauncherError::Custom(format!("创建HTTP客户端失败: {}", e))
        })?;
    
    // 优先使用国内镜像站
    let urls = [
        "https://bmclapi2.bangbang93.com/mc/game/version_manifest.json",
        "https://launchermeta.mojang.com/mc/game/version_manifest.json"
    ];

    // 获取并直接使用日志目录
    let log_file = {
        let log_dir = init_logging()?;
        log_dir.join("version_fetch.log")
    };
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .map_err(|e| LauncherError::Custom(format!("无法创建日志文件 {}: {}", log_file.display(), e)))?;
    
    // 设置日志文件权限（仅限Unix系统）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = log.metadata()?.permissions();
        perms.set_mode(0o644); // rw-r--r--
        log.set_permissions(perms)?;
    }

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
    // 创建日志文件
    let log_file = PathBuf::from("logs").join("network_debug.log");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .map_err(|e| LauncherError::Custom(format!("无法创建日志文件 {}: {}", log_file.display(), e)))?;

    writeln!(log, "[DEBUG] 准备发送请求到: {}", url)?;
    
    let request = client.get(url);
    // 获取默认请求头
    let default_headers = reqwest::header::HeaderMap::new();
    writeln!(log, "[DEBUG] 请求头: {:?}", default_headers)?;
    
    let response = request.send().await.map_err(|e| {
        let _ = writeln!(log, "[ERROR] 请求失败: {}", e);
        e
    })?;
    
    writeln!(log, "[DEBUG] 响应状态码: {}", response.status())?;
    writeln!(log, "[DEBUG] 响应头: {:?}", response.headers())?;
    
    let content_type = response.headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    writeln!(log, "[DEBUG] Content-Type: {}", content_type)?;
    
    let bytes = response.bytes().await.map_err(|e| {
        let _ = writeln!(log, "[ERROR] 读取响应体失败: {}", e);
        e
    })?;
        
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

    // 1. 获取配置和路径
    let config = load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let version_dir = game_dir.join("versions").join(&version_id);
    let (libraries_base_dir, assets_base_dir) = (
        game_dir.join("libraries"), 
        game_dir.join("assets")
    );

    // 2. 获取版本元数据
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

    // 3. 收集所有待下载的文件
    let mut downloads = Vec::new();

    // 客户端 JAR
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

    // 资源文件 (Assets)
    let assets_index_id = version_json["assetIndex"]["id"].as_str().ok_or_else(|| LauncherError::Custom("无法获取资源索引ID".to_string()))?;
    let assets_index_url = version_json["assetIndex"]["url"].as_str().ok_or_else(|| LauncherError::Custom("无法获取资源索引URL".to_string()))?;
    let assets_index_url = if is_mirror {
        assets_index_url.replace("https://launchermeta.mojang.com", base_url)
    } else {
        assets_index_url.to_string()
    };

    let assets_index_path = assets_base_dir.join("indexes").join(format!("{}.json", assets_index_id));
    fs::create_dir_all(assets_index_path.parent().unwrap())?;

    // 如果资源索引文件不存在，则从网络下载
    // 如果资源索引文件存在，则直接读取本地文件
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
            let original_url = format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash);
            let download_url = if is_mirror {
                format!("https://bmclapi2.bangbang93.com/assets/{}/{}", &hash[..2], hash)
            } else {
                original_url.clone()
            };
            let file_path = assets_base_dir.join("objects").join(&hash[..2]).join(hash);
            downloads.push(DownloadJob {
                url: download_url,
                fallback_url: if is_mirror { Some(original_url) } else { None },
                path: file_path,
                size,
            });
        }
    }

    // 库文件 (Libraries)
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
    download_all_files(downloads, &window, mirror).await?;

    // --- 5. 保存版本元数据文件 ---
    let version_json_path = version_dir.join(format!("{}.json", version_id));
    fs::write(version_json_path, text)?;

    Ok(())
}

async fn download_all_files(
    jobs: Vec<DownloadJob>,
    window: &tauri::Window,
    mirror: Option<String>,
) -> Result<(), LauncherError> {
    let total_files = jobs.len() as u64;
    download_all_files_impl(jobs, window, total_files, mirror).await
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

    // 计算游戏目录大小
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

// 辅助函数
pub fn load_config() -> Result<GameConfig, LauncherError> {
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

pub fn save_config(config: &GameConfig) -> Result<(), LauncherError> {
    let config_path = get_config_path()?;
    fs::write(config_path, serde_json::to_string_pretty(config)?)?;
    Ok(())
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



#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    println!("[DEBUG] 程序启动");
    
    // 程序启动时初始化日志
    match init_logging() {
        Ok(path) => println!("[DEBUG] 日志系统初始化完成，路径: {}", path.display()),
        Err(e) => eprintln!("[ERROR] 初始化日志失败: {}", e),
    }
    
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
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
            java::find_java_installations_command,
            java::set_java_path_command,
            load_config_key,
            save_config_key,
            java::validate_java_path,
            get_download_threads,
            set_download_threads,
            validate_version_files,
            auth::get_saved_username,
            auth::set_saved_username,
            auth::get_saved_uuid,
            auth::set_saved_uuid
        ])
        .setup(|_| {
            println!("[DEBUG] Tauri应用初始化完成");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}