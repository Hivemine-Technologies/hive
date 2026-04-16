use thiserror::Error;

#[derive(Error, Debug)]
pub enum HiveError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("git error: {0}")]
    Git(#[from] git2::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("github error: {0}")]
    GitHub(String),

    #[error("tracker error: {0}")]
    Tracker(String),

    #[error("phase error in {phase}: {message}")]
    Phase { phase: String, message: String },

    #[error("notification error: {0}")]
    Notification(String),
}

pub type Result<T> = std::result::Result<T, HiveError>;
