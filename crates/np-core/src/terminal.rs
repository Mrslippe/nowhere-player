//! 终端输出模型（ARCH-005）。
//!
//! 命令层只产出结构化值，呈现由渲染器决定；因此这里不含 HTML、ANSI 转义或布局信息。
//! 序列化形状即 IPC 契约（`camelCase` 字段名），前端手工镜像见
//! `apps/web/src/shared/terminal-model.ts`，改动须同步两侧。
//!
//! 兼容策略：未知字段被忽略；未知 `Output.type` 降级为 `text` + `[unsupported]`；
//! 未知 `status` 降级为 `pending`；未知语义色降级为默认前景色（`null`）。

use serde::{Deserialize, Deserializer, Serialize};

/// 模型版本（ARCH-005 §5）。新增字段属兼容变更；删除或改变语义需先写 ADR。
pub const SCHEMA_VERSION: u16 = 1;

/// 未知 `Output.type` 的降级文本（ARCH-005 §5）。
const UNSUPPORTED_OUTPUT_TEXT: &str = "[unsupported]";

/// 一条命令的块状态，驱动输入行左侧指示器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockStatus {
    /// 已提交、尚未开始执行。
    Pending,
    /// 执行中。
    Running,
    /// 执行成功。
    Done,
    /// 执行失败。
    Error,
    /// 被用户取消。
    Cancelled,
}

impl<'de> Deserialize<'de> for BlockStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "done" => Self::Done,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            // 未知状态按 pending 处理：不 panic，也不让整个块反序列化失败。
            _ => Self::Pending,
        })
    }
}

/// 整段文本的语义色；具体色值由渲染器决定（ARCH-005 §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// 强调色，对应 CSS 变量 `--accent`。
    Accent,
    /// 错误色，对应 `--danger`。
    Danger,
    /// 警告色，对应 `--warn`。
    Warn,
    /// 成功色，对应 `--success`。
    Success,
    /// 次要文本色，对应 `--fg-muted`。
    Muted,
}

impl Tone {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "accent" => Self::Accent,
            "danger" => Self::Danger,
            "warn" => Self::Warn,
            "success" => Self::Success,
            "muted" => Self::Muted,
            _ => return None,
        })
    }
}

/// 行内片段的语义色；具体色值由渲染器决定（ARCH-005 §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanColor {
    /// 强调色，对应 CSS 变量 `--accent`。
    Accent,
    /// 错误色，对应 `--danger`。
    Danger,
    /// 警告色，对应 `--warn`。
    Warn,
    /// 成功色，对应 `--success`。
    Success,
    /// 次要文本色，对应 `--fg-muted`。
    Muted,
}

impl SpanColor {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "accent" => Self::Accent,
            "danger" => Self::Danger,
            "warn" => Self::Warn,
            "success" => Self::Success,
            "muted" => Self::Muted,
            _ => return None,
        })
    }
}

/// 行内片段：一段文本加语义样式，不含 HTML / ANSI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    /// 内容原文。
    pub text: String,
    /// 是否加粗。
    #[serde(default)]
    pub bold: bool,
    /// 是否变暗（次要信息）。
    #[serde(default)]
    pub dim: bool,
    /// 是否斜体。
    #[serde(default)]
    pub italic: bool,
    /// 语义色；`None` 表示默认前景色（序列化为 `null`）。
    #[serde(default, deserialize_with = "deserialize_span_color")]
    pub color: Option<SpanColor>,
}

impl Span {
    /// 构造一个无样式的纯文本片段。
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            dim: false,
            italic: false,
            color: None,
        }
    }
}

/// `media.kind`：封面或缩略图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// 专辑 / 单曲封面。
    Cover,
    /// 列表缩略图。
    Thumb,
}

impl<'de> Deserialize<'de> for MediaKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "thumb" => Self::Thumb,
            // 含 "cover" 与未来新增的未知值：降级为封面，不让整块失败。
            _ => Self::Cover,
        })
    }
}

/// 输出原语。`type` 标签为 `snake_case`；未知类型反序列化时降级为
/// `Text` + [`UNSUPPORTED_OUTPUT_TEXT`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    /// 普通文本、键值行。
    Text {
        /// 行内片段，按顺序拼接。
        spans: Vec<Span>,
        /// 整段默认语义色；`None` 为默认前景色。
        tone: Option<Tone>,
    },
    /// 搜索结果、队列、库列表等表格。
    Table {
        /// 表头，按列顺序。
        columns: Vec<String>,
        /// 行数据；每行长度应与 `columns` 一致。
        rows: Vec<Vec<String>>,
        /// 行是否可选中（悬停 / 点击）。
        selectable: bool,
    },
    /// 扫描、下载等可原地刷新的进度。
    Progress {
        /// 进度描述，如 `扫描 ~/Music`。
        label: String,
        /// 当前值；单位由 `unit` 决定。
        current: u64,
        /// 总量；`0` 表示总量未知。
        total: u64,
        /// 单位，如 `首` / `bytes`。
        unit: String,
    },
    /// 失败输出。
    Error {
        /// 稳定错误码，如 `E_LIB_PERMISSION`。
        code: String,
        /// 一句话原因。
        message: String,
        /// 修复建议，如可直接执行的命令。
        hint: Option<String>,
        /// 可点击的建议动作（按钮文本；命令绑定留待后续卡片）。
        actions: Option<Vec<String>>,
    },
    /// 补全建议 / 纠错提示。
    Hint {
        /// 提示文本。
        text: String,
        /// 采纳方式，如 `按 Tab 采纳`。
        suggestion: Option<String>,
    },
    /// 封面 / 缩略图；xterm 渲染器降级为文字占位。
    Media {
        /// 图片类型。
        kind: MediaKind,
        /// 资源引用；具体协议由上层约定。
        #[serde(rename = "ref")]
        r#ref: String,
        /// 期望边长（像素）；结构化渲染当前按 48 处理。
        size: u32,
    },
}

impl Output {
    /// 返回序列化后的 `type` 标签，便于渲染器分派。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Table { .. } => "table",
            Self::Progress { .. } => "progress",
            Self::Error { .. } => "error",
            Self::Hint { .. } => "hint",
            Self::Media { .. } => "media",
        }
    }
}

/// 与 [`Output`] 同形的反序列化中转类型；`Unsupported` 捕获未知 `type`。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputWire {
    Text {
        spans: Vec<Span>,
        #[serde(default, deserialize_with = "deserialize_tone")]
        tone: Option<Tone>,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        selectable: bool,
    },
    Progress {
        label: String,
        current: u64,
        total: u64,
        unit: String,
    },
    Error {
        code: String,
        message: String,
        #[serde(default)]
        hint: Option<String>,
        #[serde(default)]
        actions: Option<Vec<String>>,
    },
    Hint {
        text: String,
        #[serde(default)]
        suggestion: Option<String>,
    },
    Media {
        kind: MediaKind,
        #[serde(rename = "ref")]
        r#ref: String,
        size: u32,
    },
    /// 未知 `type`：保留反序列化成功，由 [`Output::from`] 降级为 text。
    #[serde(other)]
    Unsupported,
}

impl From<OutputWire> for Output {
    fn from(wire: OutputWire) -> Self {
        match wire {
            OutputWire::Text { spans, tone } => Self::Text { spans, tone },
            OutputWire::Table {
                columns,
                rows,
                selectable,
            } => Self::Table {
                columns,
                rows,
                selectable,
            },
            OutputWire::Progress {
                label,
                current,
                total,
                unit,
            } => Self::Progress {
                label,
                current,
                total,
                unit,
            },
            OutputWire::Error {
                code,
                message,
                hint,
                actions,
            } => Self::Error {
                code,
                message,
                hint,
                actions,
            },
            OutputWire::Hint { text, suggestion } => Self::Hint { text, suggestion },
            OutputWire::Media { kind, r#ref, size } => Self::Media { kind, r#ref, size },
            OutputWire::Unsupported => Self::Text {
                spans: vec![Span::plain(UNSUPPORTED_OUTPUT_TEXT)],
                tone: None,
            },
        }
    }
}

impl<'de> Deserialize<'de> for Output {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        OutputWire::deserialize(deserializer).map(Output::from)
    }
}

/// 一条命令 = 一个块。渲染器按 `outputs` 顺序绘制，进度通过 `id` 原地替换。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// 稳定标识，用于增量更新（进度条改写同一块）。
    pub id: String,
    /// 用户原始输入（含参数）。
    pub input: String,
    /// 开始时间，Unix 毫秒时间戳。
    pub started_at: u64,
    /// 结束耗时（毫秒）；运行中为 `None`（序列化为 `null`）。
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// 当前状态，驱动输入行指示器。
    pub status: BlockStatus,
    /// 输出区，按顺序渲染。
    pub outputs: Vec<Output>,
    /// 尾注，如“3 条结果 · 输入 add 1-2 加入队列”。
    #[serde(default)]
    pub footer: Option<String>,
    /// 载荷版本；当前恒为 [`SCHEMA_VERSION`]。
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
}

fn default_schema_version() -> u16 {
    SCHEMA_VERSION
}

fn deserialize_tone<'de, D>(deserializer: D) -> Result<Option<Tone>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(Tone::from_name))
}

fn deserialize_span_color<'de, D>(deserializer: D) -> Result<Option<SpanColor>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(SpanColor::from_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_block() -> Block {
        Block {
            id: "blk_demo".to_string(),
            input: "scan ~/Music".to_string(),
            started_at: 1_762_512_000_000,
            duration_ms: Some(1_842),
            status: BlockStatus::Done,
            outputs: vec![
                Output::Text {
                    spans: vec![
                        Span {
                            text: "扫描完成".to_string(),
                            bold: true,
                            dim: false,
                            italic: false,
                            color: Some(SpanColor::Accent),
                        },
                        Span {
                            text: " · 1,284 首".to_string(),
                            bold: false,
                            dim: true,
                            italic: false,
                            color: Some(SpanColor::Muted),
                        },
                    ],
                    tone: None,
                },
                Output::Table {
                    columns: vec![
                        "#".to_string(),
                        "曲名".to_string(),
                        "艺人".to_string(),
                        "时长".to_string(),
                    ],
                    rows: vec![
                        vec![
                            "1".to_string(),
                            "晴天".to_string(),
                            "周杰伦".to_string(),
                            "4:29".to_string(),
                        ],
                        vec![
                            "2".to_string(),
                            "稻香".to_string(),
                            "周杰伦".to_string(),
                            "3:43".to_string(),
                        ],
                    ],
                    selectable: true,
                },
                Output::Progress {
                    label: "扫描 ~/Music".to_string(),
                    current: 1_284,
                    total: 1_284,
                    unit: "首".to_string(),
                },
                Output::Error {
                    code: "E_LIB_PERMISSION".to_string(),
                    message: "无法读取目录 ~/Music/Private".to_string(),
                    hint: Some("检查文件权限后重试".to_string()),
                    actions: Some(vec!["重试".to_string(), "跳过该目录".to_string()]),
                },
                Output::Hint {
                    text: "did you mean 'play'?".to_string(),
                    suggestion: Some("按 Tab 采纳".to_string()),
                },
                Output::Media {
                    kind: MediaKind::Cover,
                    r#ref: "np://library/album/1/cover.jpg".to_string(),
                    size: 48,
                },
            ],
            footer: Some("6 项输出 · 输入 play 1 开始播放".to_string()),
            schema_version: SCHEMA_VERSION,
        }
    }

    #[test]
    fn block_serialization_roundtrips() {
        let block = sample_block();
        let json = serde_json::to_string(&block).expect("serialize block");

        // 契约要求 camelCase 字段名（ARCH-005 §2.1 / §5）。
        assert!(
            json.contains(r#""schemaVersion":1"#),
            "schemaVersion missing: {json}"
        );
        assert!(
            json.contains(r#""startedAt":1762512000000"#),
            "startedAt missing: {json}"
        );
        assert!(
            json.contains(r#""durationMs":1842"#),
            "durationMs missing: {json}"
        );
        assert!(
            json.contains(r#""type":"text""#),
            "type tag missing: {json}"
        );
        assert!(
            json.contains(r#""ref":"np://"#),
            "ref field missing: {json}"
        );

        let parsed: Block = serde_json::from_str(&json).expect("deserialize block");
        assert_eq!(parsed, block);
    }

    #[test]
    fn unknown_fields_and_variants_degrade() {
        let json = r#"{
            "id": "blk_unknown",
            "input": "search 晴天",
            "startedAt": 1,
            "durationMs": null,
            "status": "paused",
            "outputs": [{
                "type": "text",
                "tone": "purple",
                "spans": [{
                    "text": "hi",
                    "bold": true,
                    "dim": false,
                    "italic": false,
                    "color": "neon",
                    "unknown_span_field": 1
                }],
                "unknown_output_field": true
            }],
            "footer": null,
            "schemaVersion": 1,
            "unknown_block_field": {"a": 1}
        }"#;

        let block: Block =
            serde_json::from_str(json).expect("unknown fields/variants must not fail");
        assert_eq!(block.status, BlockStatus::Pending);
        assert_eq!(block.outputs.len(), 1);
        match &block.outputs[0] {
            Output::Text { spans, tone } => {
                assert_eq!(*tone, None);
                assert_eq!(spans.len(), 1);
                assert_eq!(spans[0].text, "hi");
                assert!(spans[0].bold);
                assert_eq!(spans[0].color, None);
            }
            other => panic!("expected text output, got {}", other.kind()),
        }
    }

    #[test]
    fn unknown_output_type_degrades_to_unsupported_text() {
        let output: Output = serde_json::from_str(r#"{"type":"chart","series":[1,2,3]}"#)
            .expect("unknown output type must not fail");

        assert_eq!(output.kind(), "text");
        match output {
            Output::Text { spans, tone } => {
                assert_eq!(tone, None);
                assert_eq!(spans.len(), 1);
                assert_eq!(spans[0].text, UNSUPPORTED_OUTPUT_TEXT);
                assert!(!spans[0].bold && !spans[0].dim && !spans[0].italic);
                assert_eq!(spans[0].color, None);
            }
            other => panic!("expected degraded text output, got {}", other.kind()),
        }
    }

    #[test]
    fn demo_fixture_deserializes_and_covers_six_outputs() {
        let raw = include_str!("../../../apps/web/src/shared/fixtures/terminal-demo.json");
        let block: Block =
            serde_json::from_str(raw).expect("terminal-demo.json must deserialize as Block");

        assert_eq!(block.schema_version, SCHEMA_VERSION);
        let kinds: Vec<&str> = block.outputs.iter().map(Output::kind).collect();
        assert_eq!(
            kinds,
            ["text", "table", "progress", "error", "hint", "media"]
        );

        match &block.outputs[1] {
            Output::Table {
                columns,
                rows,
                selectable,
            } => {
                assert_eq!(columns.len(), 4);
                assert_eq!(rows.len(), 5);
                assert!(rows.iter().all(|row| row.len() == columns.len()));
                assert!(*selectable);
            }
            other => panic!("expected table output, got {}", other.kind()),
        }

        match &block.outputs[3] {
            Output::Error { code, hint, .. } => {
                assert_eq!(code, "E_LIB_PERMISSION");
                assert!(hint.is_some());
            }
            other => panic!("expected error output, got {}", other.kind()),
        }

        match &block.outputs[5] {
            Output::Media { kind, r#ref, size } => {
                assert_eq!(*kind, MediaKind::Cover);
                assert!(r#ref.starts_with("np://"));
                assert_eq!(*size, 48);
            }
            other => panic!("expected media output, got {}", other.kind()),
        }
    }
}
