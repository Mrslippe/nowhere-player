//! 错误类型

use std::path::PathBuf;

/// `np-library` 的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// SQLite 层面的错误（打开、迁移、读写）。
    #[error("数据库错误: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// 文件系统错误。
    #[error("文件系统错误: {0}")]
    Io(#[from] std::io::Error),

    /// 单个文件的标签读取失败。扫描器把它计入 `failed` 后继续。
    #[error("无法读取标签 ({path}): {message}")]
    Tag {
        /// 出错的文件。
        path: PathBuf,
        /// 底层错误描述。
        message: String,
    },

    /// 后台线程异常终止。
    #[error("扫描线程异常终止")]
    WorkerPanicked,
}

/// crate 内部使用的 `Result` 别名。
pub type Result<T> = std::result::Result<T, LibraryError>;
