use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::Emitter;
use md5::Md5;
use digest::Digest;
use uuid::Uuid;

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

use crate::error::LauncherError;
use crate::models::*;

/// 启动 Minecraft 游戏
#[tauri::command]
pub async fn launch_minecraft(options: LaunchOptions, window: tauri::Window) -> Result<(), LauncherError> {
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
            max_memory: 4096, // 默认最大内存为4GB
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

fn get_config_path() -> Result<PathBuf, LauncherError> {
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent()
        .ok_or_else(|| LauncherError::Custom("无法获取可执行文件目录".to_string()))?;
    
    Ok(exe_dir.join("ar1s.json"))
}