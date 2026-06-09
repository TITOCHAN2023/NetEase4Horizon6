use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 配置文件，保存在 exe 同目录的 config.toml。
/// 第一次运行会自动生成；用户也可以手动改。
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// 要采集的进程名（网易云音乐固定是 cloudmusic.exe）
    pub process_name: String,
    /// HTTP 流监听地址。默认只在本机可见；想让局域网其它设备也能听，用 0.0.0.0
    pub bind: String,
    /// HTTP 流端口
    pub port: u16,
    /// MP3 码率（kbps）：128 / 160 / 192 / 256 / 320
    pub bitrate_kbps: u32,
    /// 上次安装 mod 的游戏目录（自动记住，下次免询问）
    pub game_dir: Option<String>,
    /// 启动后是否自动打开 mod 的 Web UI 并把流地址复制到剪贴板
    pub open_ui: bool,
    /// mod Web UI 的端口（mod 默认 8103）
    pub mod_ui_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            process_name: "cloudmusic.exe".into(),
            bind: "127.0.0.1".into(),
            port: 8123,
            bitrate_kbps: 192,
            game_dir: None,
            open_ui: true,
            mod_ui_port: 8103,
        }
    }
}

fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("config.toml")))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

impl Config {
    pub fn load() -> Self {
        match std::fs::read_to_string(config_path()) {
            Ok(s) => toml::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(config_path(), s);
        }
    }
}
