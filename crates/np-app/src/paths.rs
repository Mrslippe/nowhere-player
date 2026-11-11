//! 宿主注入的目录；命令层不直接读 Tauri 的 path API。

use std::path::PathBuf;

/// 命令层需要的三个目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// 设置、主题等配置。
    pub config_dir: PathBuf,
    /// 在线音频/封面缓存。
    pub cache_dir: PathBuf,
    /// 媒体库与封面。
    pub data_dir: PathBuf,
}
