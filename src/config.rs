use std::path::PathBuf;

use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub fps: u32,
    pub output_dir: PathBuf,
    pub monitor_index: usize,
    pub system_audio: bool,
    pub microphone: bool,
    pub hotkey: String,
    pub quality: u8,
    pub max_height: Option<u32>,
    /// Forces exact 1920x1080 output at 60 fps regardless of the source
    /// monitor's native resolution/refresh (letterboxed if aspect differs).
    /// Overrides `fps` and `max_height` while enabled.
    pub preset_1080p60: bool,
}

impl Config {
    /// The fps actually used for capture/encode: forced to 60 when the
    /// 1920x1080@60 preset is enabled, otherwise the configured `fps`.
    pub fn effective_fps(&self) -> u32 {
        if self.preset_1080p60 { 60 } else { self.fps }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps: 30,
            output_dir: default_output_dir(),
            monitor_index: 0,
            system_audio: true,
            microphone: true,
            hotkey: "ctrl+alt+r".to_string(),
            quality: 23,
            max_height: None,
            preset_1080p60: false,
        }
    }
}

fn default_output_dir() -> PathBuf {
    if let Some(ud) = UserDirs::new() {
        if let Some(v) = ud.video_dir() {
            return v.to_path_buf();
        }
        return ud.home_dir().to_path_buf();
    }
    PathBuf::from(".")
}

pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("com", "litecap", "litecap")
}

fn config_path() -> Option<PathBuf> {
    project_dirs().map(|p| p.config_dir().join("config.toml"))
}

pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            log::warn!("no project dirs available, using default config");
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::warn!("failed to parse config at {path:?}: {e}, using defaults");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = config_path() else {
            anyhow::bail!("no project dirs available");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }
}
