//! 命令注册表与解析器（T-0017）。
//!
//! 这一层只把用户输入变成结构化命令：定义命令、分词、解析、校验参数形状。
//! 它不执行命令、不碰 IPC、不依赖任何第三方解析库。
//!
//! # 与 POSIX shell 的有意差异
//!
//! - **反斜杠是字面量，不是转义符**。`C:\Music\a.flac` 原样保留一个 token，
//!   `"C:\Music\My Music\a.flac"` 里的反斜杠也不会被吞掉。
//! - **引号内不做任何转义**，引号必须在同一个 token 内闭合；未闭合报
//!   [`CommandErrorCode::Arg`]（`E_CMD_ARG`），提示“引号未闭合”。
//! - 空白是唯一的分隔符；连续空白折叠。空输入 / 纯空白返回 `None`，不是错误。
//!
//! # 解析层只管形状
//!
//! `vol 999`、`seek 1:30` 都会解析成功；取值范围、当前状态是否允许执行等
//! 语义判断留给执行层。参数个数、整数/位置格式、枚举取值属于形状，在这里校验。

mod registry;
mod tokenizer;

use std::fmt;

pub use registry::{ArgKind, ArgSpec, CommandSpec, FlagSpec, Registry, SubcommandSpec, registry};
pub(crate) use tokenizer::{Token, tokenize};

/// 命令解析错误码闭集，与 [`crate::PlayerErrorCode`] 同构（`E_` 前缀即 IPC 字符串）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommandErrorCode {
    /// 未知命令或子命令；能算出来时附带拼写建议。
    #[serde(rename = "E_CMD_UNKNOWN")]
    Unknown,
    /// 参数 / 标志 / 引号形状错误；附带 usage。
    #[serde(rename = "E_CMD_ARG")]
    Arg,
}

impl CommandErrorCode {
    /// 稳定的 IPC / CLI 字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "E_CMD_UNKNOWN",
            Self::Arg => "E_CMD_ARG",
        }
    }
}

impl fmt::Display for CommandErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 可直接展示给用户的解析错误。
///
/// `message` 是一句短句；`usage` 只在能从命令表推出来时给出；`suggestion`
/// 是规范命令名（而不是用户输入的别名/大小写），例如 `plya` → `play`。
/// [`Display`] 会把三者拼成终端里可直接显示的多行文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 稳定错误码。
    pub code: CommandErrorCode,
    /// 一句话原因，可直接展示。
    pub message: String,
    /// 该命令的用法，如 `seek <ms|mm:ss|±s>`。
    pub usage: Option<String>,
    /// 拼写建议的规范写法；只对 `E_CMD_UNKNOWN` 出现。
    pub suggestion: Option<String>,
}

impl ParseError {
    fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            usage: None,
            suggestion: None,
        }
    }

    pub(crate) fn unclosed_quote(quote: char) -> Self {
        Self::new(
            CommandErrorCode::Arg,
            format!("引号未闭合：缺少结尾的 {quote}"),
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(suggestion) = &self.suggestion {
            write!(f, "（did you mean '{suggestion}'?）")?;
        }
        if let Some(usage) = &self.usage {
            write!(f, "\n用法：{usage}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}
