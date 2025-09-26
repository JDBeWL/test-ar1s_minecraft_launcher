use std::fs;


use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{Window, Emitter, Listener};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use reqwest;


use crate::error::LauncherError;


use crate::models::DownloadJob;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadState {
    completed_files: Vec<String>,
    failed_files: Vec<String>,
    active_downloads: HashMap<String, std::path::PathBuf>,
}



#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Downloading,
    Completed,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub progress: u64,      // 已完成文件数
    pub total: u64,        // 总文件数
    pub speed: f64,        // 下载速度(KB/s)
    pub status: DownloadStatus,
    pub bytes_downloaded: u64, // 已下载字节数
    pub total_bytes: u64,  // 总字节数
    pub percent: u8,       // 完成百分比(0-100)
}

pub async fn download_all_files(
    jobs: Vec<DownloadJob>,
    window: &Window,
    _total_files: u64,
    _mirror: Option<String>,
) -> Result<(), LauncherError> {
    let config = crate::load_config()?;
    let threads = config.download_threads as usize;

    // 获取版本ID（从第一个下载任务的路径推断）
    let version_id = jobs.first()
        .and_then(|j| j.path.parent())
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    // 创建版本特定的状态文件
    let state_file = std::env::temp_dir()
        .join(format!("ar1s_download_state_{}.json", version_id));
    let download_state = Arc::new(Mutex::new(
        if state_file.exists() {
            serde_json::from_str(&std::fs::read_to_string(&state_file)?)?
        } else {
            DownloadState {
                completed_files: Vec::new(),
                failed_files: Vec::new(),
                active_downloads: HashMap::new(),
            }
        }
    ));

    // 创建过滤后的任务列表（不移动原始jobs）
    let filtered_jobs: Vec<DownloadJob> = jobs.iter()
        .filter(|job| !download_state.lock().unwrap().completed_files.contains(&job.url))
        .cloned()
        .collect();

    // 更新总文件数为实际需要下载的数量
    let actual_total = jobs.len() as u64;

    let completed_count_from_state = download_state.lock().unwrap().completed_files.len() as u64;

    // 创建共享状态
    // TODO: 这里的状态应该改为一个结构体，而不是使用原子类型，以便更好地跟踪状态
    let files_downloaded = Arc::new(AtomicU64::new(completed_count_from_state));
    let bytes_downloaded = Arc::new(AtomicU64::new(0));
    let bytes_since_last = Arc::new(AtomicU64::new(0));
    let state = Arc::new(AtomicBool::new(true)); // true = running, false = cancelled/stopped
    let was_cancelled = Arc::new(AtomicBool::new(false));
    let error_occurred = Arc::new(tokio::sync::Mutex::new(None::<String>));

    // 监听取消下载事件
    let state_clone = state.clone();
    let was_cancelled_clone = was_cancelled.clone();
    window.once("cancel-download", move |_| {
        state_clone.store(false, Ordering::SeqCst);
        was_cancelled_clone.store(true, Ordering::SeqCst);
    });

    // 创建进度报告器
    let reporter_handle = {
        let files_downloaded = files_downloaded.clone();
        let bytes_downloaded = bytes_downloaded.clone();
        let bytes_since_last = bytes_since_last.clone();
        let state = state.clone();
        let window = window.clone();
        let report_interval = Duration::from_millis(200); // 更频繁的更新
        let total_size = jobs.iter().map(|j| j.size).sum::<u64>();

        tokio::spawn(async move {
            while state.load(Ordering::SeqCst) {
                tokio::time::sleep(report_interval).await;
                if !state.load(Ordering::SeqCst) { break; }

                let downloaded_count = files_downloaded.load(Ordering::SeqCst);
                let current_bytes = bytes_downloaded.load(Ordering::SeqCst);
                let bytes_since = bytes_since_last.swap(0, Ordering::SeqCst);
                let speed = (bytes_since as f64 / 1024.0) / report_interval.as_secs_f64();
                let progress_percent = if total_size > 0 {
                    (current_bytes as f64 / total_size as f64 * 100.0).round() as u8
                } else { 0 };

                let progress = DownloadProgress {
                    progress: downloaded_count,
                    total: actual_total,
                    speed,
                    status: DownloadStatus::Downloading,
                    bytes_downloaded: current_bytes,
                    total_bytes: total_size,
                    percent: progress_percent,
                };
                let _ = window.emit("download-progress", &progress);
            }
        })
    };

    // 创建线程池
    let semaphore = Arc::new(tokio::sync::Semaphore::new(threads));
    let mut handles = vec![];

    // 在循环前克隆共享状态
    let state_file_clone = state_file.clone();


    for job in filtered_jobs {
        if !state.load(Ordering::SeqCst) { break; }

        // 记录正在进行的下载
        {
            let mut state = download_state.lock().unwrap();
            state.active_downloads.insert(job.url.clone(), job.path.clone());
            std::fs::write(&state_file_clone, serde_json::to_string(&*state)?)?;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let state = state.clone();
        let files_downloaded = files_downloaded.clone();
        let bytes_downloaded = bytes_downloaded.clone();
        let bytes_since_last = bytes_since_last.clone();
        let error_occurred = error_occurred.clone();
        let job_state_file = state_file.clone();
        let job_download_state = download_state.clone();

        handles.push(tokio::spawn(async move {
            let mut current_job_error: Option<LauncherError> = None;
            let mut job_succeeded = false;

            // 检查文件是否已存在，如果存在且大小匹配，则跳过下载
            // TODO: 这里应该检查文件的SHA256哈希值，而不是大小
            if job.path.exists() {
                if let Ok(metadata) = fs::metadata(&job.path) {
                    if metadata.len() == job.size {
                        println!("DEBUG: 文件已存在且大小匹配，跳过下载: {}", job.path.display());
                        files_downloaded.fetch_add(1, Ordering::SeqCst);
                        bytes_downloaded.fetch_add(job.size, Ordering::SeqCst);
                        job_succeeded = true;
                    }
                }
            }

            // 如果文件不存在或大小不匹配，则开始下载
            if !job_succeeded {
                const MAX_JOB_RETRIES: usize = 3;
                for retry in 0..MAX_JOB_RETRIES {
                    if !state.load(Ordering::SeqCst) { break; }
                    println!("DEBUG: 尝试下载文件: {} (重试 {}/{})", job.url, retry + 1, MAX_JOB_RETRIES);
                    match download_file(&job, &state, &bytes_downloaded, &bytes_since_last).await {
                        Ok(_) => {
                            files_downloaded.fetch_add(1, Ordering::SeqCst);
                            current_job_error = None;
                            job_succeeded = true; // 下载成功
                            break;
                        }
                        Err(e) => {
                            println!("ERROR: 文件下载失败: {} (重试 {}/{}) - {}", job.url, retry + 1, MAX_JOB_RETRIES, e);
                            current_job_error = Some(e);
                            tokio::time::sleep(Duration::from_secs(1 << retry)).await; // 指数退避
                        }
                    }
                }
            }

            // 克隆共享状态，以便在下载完成时更新
            // TODO: 这里应该使用一个结构体来跟踪状态，而不是使用原子类型，以便更好地跟踪状态
            let state_file_clone = job_state_file;
            let download_state_clone = job_download_state;

            if job_succeeded {
                // 记录成功下载
                let mut state = download_state_clone.lock().unwrap();
                state.completed_files.push(job.url.clone());
                state.active_downloads.remove(&job.url);
                std::fs::write(&state_file_clone, serde_json::to_string(&*state)?)?;
            } else { // 下载失败
                if let Some(e) = current_job_error {
                    state.store(false, Ordering::SeqCst);
                    let mut error_guard = error_occurred.lock().await;
                    if error_guard.is_none() {
                        *error_guard = Some(e.to_string());
                    }

                    // 记录失败下载
                    let mut state = download_state_clone.lock().unwrap();
                    state.failed_files.push(job.url.clone());
                    state.active_downloads.remove(&job.url);
                    std::fs::write(&state_file_clone, serde_json::to_string(&*state)?)?;
                }
            }
            drop(permit);
            Ok::<(), LauncherError>(())
        }));
    }

    // 等待所有线程完成
    for handle in handles {
        let _ = handle.await;
    }

    // 取消下载
    state.store(false, Ordering::SeqCst);
    reporter_handle.await?;

    if was_cancelled.load(Ordering::SeqCst) {
        let _ = window.emit("download-progress", &DownloadProgress {
            progress: files_downloaded.load(Ordering::SeqCst),
            total: actual_total,
            speed: 0.0,
            status: DownloadStatus::Cancelled,
            bytes_downloaded: bytes_downloaded.load(Ordering::SeqCst),
            total_bytes: jobs.iter().map(|j| j.size).sum(),
            percent: 0,
        });
        return Err(LauncherError::Custom("下载已取消".to_string()));
    }

    if let Some(err_msg) = error_occurred.lock().await.take() {
        let _ = window.emit("download-progress", &DownloadProgress {
            progress: files_downloaded.load(Ordering::SeqCst),
            total: actual_total,
            speed: 0.0,
            status: DownloadStatus::Error,
            bytes_downloaded: bytes_downloaded.load(Ordering::SeqCst),
            total_bytes: jobs.iter().map(|j| j.size).sum(),
            percent: 0,
        });
        return Err(LauncherError::Custom(err_msg));
    }
    
    // 下载完成
    let _ = window.emit("download-progress", &DownloadProgress {
        progress: actual_total,
        total: actual_total,
        speed: 0.0,
        status: DownloadStatus::Completed,
        bytes_downloaded: bytes_downloaded.load(Ordering::SeqCst),
        total_bytes: jobs.iter().map(|j| j.size).sum(),
        percent: 100,
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

    // 下载文件
    match download_chunk(&client, &job.url, job, state, bytes_downloaded, bytes_since_last).await {
        Ok(_) => return Ok(()),
        Err(e) => {
            // 如果存在回退URL，则尝试使用回退URL下载
            if let Some(fallback_url) = &job.fallback_url {
                let is_http_error = if let LauncherError::Http(err) = &e {
                    err.status() == Some(reqwest::StatusCode::NOT_FOUND) || err.is_timeout()
                } else {
                    false
                };
                let is_mismatch_error = e.to_string().contains("File size mismatch");

                if is_http_error || is_mismatch_error {
                    println!("DEBUG: Primary URL {} failed ({}), trying fallback: {}", job.url, e, fallback_url);
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
            println!("ERROR: 文件下载失败: {}", e);
            return Err(e);
        }
    }
}

async fn download_chunk(
    client: &reqwest::Client,
    url: &str, // 下载URL
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
) -> Result<(), LauncherError> {

    // 创建父目录
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

    // 检查文件大小
    let actual_size = file.metadata().await?.len();
    if actual_size != job.size {
        return Err(LauncherError::Custom(format!(
            "File size mismatch for {}: expected {}, got {}",
            job.path.display(), job.size, actual_size
        )));
    }

    Ok(())
}