use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tauri::command;
use crate::{LauncherError, load_config, save_config};

// 查找Java安装路径
#[command]
pub async fn find_java_installations_command() -> Result<Vec<String>, LauncherError> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\\Program Files".into());
        let program_files_x86 =
            std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\\Program Files (x86)".into());

        // 检查常见Java安装路径
        let java_dirs = vec![
            format!("{}\\Java", program_files),
            format!("{}\\Java", program_files_x86),
            r"C:\\Program Files\\Java".to_string(),
            r"C:\\Program Files (x86)\\Java".to_string(),
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
#[command]
pub async fn set_java_path_command(path: String) -> Result<(), LauncherError> {
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

#[command]
pub async fn validate_java_path(path: String) -> Result<bool, LauncherError> {
    let java_exe = PathBuf::from(&path);
    if java_exe.is_file() {
        // 检查java.exe是否存在
        let output = Command::new(&java_exe)
            .arg("-version")
            .output();

        match output {
            Ok(out) => {
                // 检查stderr中是否包含"java version"或"openjdk version"字符串
                let stderr_str = String::from_utf8_lossy(&out.stderr);
                Ok(out.status.success() && (stderr_str.contains("java version") || stderr_str.contains("openjdk version")))
            },
            Err(_) => Ok(false),
        }
    } else if path.to_lowercase() == "java" {
        // 检查Java路径
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
