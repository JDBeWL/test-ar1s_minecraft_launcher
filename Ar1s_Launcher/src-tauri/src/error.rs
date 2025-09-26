use std::io;
use thiserror::Error;
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

impl From<JoinError> for LauncherError {
    fn from(err: JoinError) -> Self {
        LauncherError::Custom(format!("Task join error: {}", err))
    }
}