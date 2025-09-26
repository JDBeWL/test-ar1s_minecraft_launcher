use crate::{LauncherError, load_config, save_config};
use tauri::command;

// 获取保存的用户名
#[allow(dead_code)]
#[command]
pub async fn get_saved_username() -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    Ok(config.username)
}

// 设置保存的用户名
#[allow(dead_code)]
#[command]
pub async fn set_saved_username(username: String) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.username = Some(username);
    save_config(&config)?;
    Ok(())
}

// 获取保存的UUID
#[allow(dead_code)]
#[command]
pub async fn get_saved_uuid() -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    Ok(config.uuid)
}

// 设置保存的UUID
#[allow(dead_code)]
#[command]
pub async fn set_saved_uuid(uuid: String) -> Result<(), LauncherError> {
    let mut config = load_config()?;
    config.uuid = Some(uuid);
    save_config(&config)?;
    Ok(())
}
