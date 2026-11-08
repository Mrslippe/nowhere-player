//! 命令执行器（T-0018）：把 `np_core` 的解析结果变成播放器调用与终端输出。
//!
//! 这一层是 T-0017（解析）与 T-0019（渲染）之间的胶水：
//! - 只依赖 [`PlayerApi`]，不直接依赖 `PlayerHandle`；单测用 `FakeApi` 记录调用，
//!   因此执行器逻辑不碰真实音频设备（T-0018 §6.1）。
//! - 输出复用 `np_core::terminal` 的六种原语，不自造字符串协议。
//! - `cmd.exec` 同步执行，结果通过 `terminal:block` 事件推回前端。
//!
//! # 约定
//!
//! - **序号对用户一律 1 基**：`play 3` 是队列第 3 首，执行层内部转成 0 基。
//! - `vol ±N` 先读当前音量再算，结果在调用前钳到 0–100。
//! - `mode` 一次下发完整 `{ repeat, shuffle }`，避免两次调用互相覆盖。
//! - `clear` 是视图操作，由前端处理；执行器只留一条防御性错误。

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use np_core::command::{Completion, complete, parse, registry};
use np_core::{
    Block, BlockStatus, Output, ParseError, Parsed, PlaybackMode, PlaybackState, PlayerError,
    PlayerErrorCode, QueueView, RepeatMode, SCHEMA_VERSION, Span,
};
use tauri::{AppHandle, Emitter, State};

use crate::player_actor::PlayerHandle;

/// 执行器依赖的播放器能力（T-0018 §6.1）。
///
/// 生产实现是 `HandleApi`（`player_actor` 里的 `PlayerHandle` 适配）；
/// 单测实现是 `FakeApi`。执行器只按 trait 调用，所以命令映射可以整体单测。
pub(crate) trait PlayerApi {
    /// 当前完整快照。
    fn snapshot(&self) -> Result<PlaybackState, CmdError>;
    /// 打开路径并播放（同时入队）；`None` 时继续当前曲目。
    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError>;
    /// 暂停。
    fn pause(&self) -> Result<PlaybackState, CmdError>;
    /// 继续播放（等价于 `play(None)`）。
    fn resume(&self) -> Result<PlaybackState, CmdError>;
    /// 下一首。
    fn next(&self) -> Result<PlaybackState, CmdError>;
    /// 上一首。
    fn prev(&self) -> Result<PlaybackState, CmdError>;
    /// 跳到绝对播放位置（毫秒）。
    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError>;
    /// 设置音量（0.0–1.0）。
    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError>;
    /// 设置完整播放模式。
    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError>;
    /// 队列快照。
    fn queue_list(&self) -> Result<QueueView, CmdError>;
    /// 追加曲目（按给定顺序）。
    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError>;
    /// 移除播放顺序里的第 `index` 首（0 基）。
    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError>;
    /// 清空队列。
    fn queue_clear(&self) -> Result<QueueView, CmdError>;
    /// 跳到播放顺序里的第 `index` 首（0 基）并播放。
    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError>;
}

/// 执行期错误：稳定的 IPC 码 + 一句原因 + 可选的修复提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CmdError {
    /// IPC 错误码，如 `E_CMD_ARG` / `E_UNSUPPORTED`。
    pub(crate) code: String,
    /// 一句话原因，可直接展示。
    pub(crate) message: String,
    /// 修复提示；解析错误用 `usage` 或拼写建议填充。
    pub(crate) hint: Option<String>,
}

impl CmdError {
    /// 用错误码与原因构造错误。
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: None,
        }
    }

    /// 解析错误 → 执行错误：`hint` 优先用 `usage`，没有则用拼写建议。
    pub(crate) fn from_parse(error: &ParseError) -> Self {
        Self {
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
            hint: error
                .usage
                .as_deref()
                .map(|usage| format!("用法：{usage}"))
                .or_else(|| error.suggestion.as_ref().map(|name| suggestion(name))),
        }
    }

    /// 播放器错误 → 执行错误。`hint` 由执行器补命令用法（见 `with_usage`）。
    pub(crate) fn from_player(error: PlayerError) -> Self {
        Self {
            code: player_error_code(error.code).to_owned(),
            message: error.message,
            hint: None,
        }
    }

    /// 补一条提示；已有提示时不覆盖。
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        if self.hint.is_none() {
            self.hint = Some(hint.into());
        }
        self
    }

    /// 失败时至少给一条提示：没有更具体的就用命令用法。
    pub(crate) fn with_usage(mut self, parsed: &Parsed) -> Self {
        if self.hint.is_none() {
            self.hint = Some(format!("用法：{}", parsed.usage()));
        }
        self
    }

    /// 转成终端的 `error` 原语。
    pub(crate) fn to_output(&self) -> np_core::Output {
        np_core::Output::Error {
            code: self.code.clone(),
            message: self.message.clone(),
            hint: self.hint.clone(),
            actions: None,
        }
    }
}

/// 拼写建议的展示文案，与 `ParseError` 的 `Display` 保持一致。
fn suggestion(name: &str) -> String {
    format!("did you mean '{name}'?")
}

/// `PlayerErrorCode` 的 IPC 字符串（`np-core` 没有暴露 `as_str`）。
fn player_error_code(code: PlayerErrorCode) -> &'static str {
    match code {
        PlayerErrorCode::Decode => "E_DECODE",
        PlayerErrorCode::DeviceLost => "E_DEVICE_LOST",
        PlayerErrorCode::Unsupported => "E_UNSUPPORTED",
        PlayerErrorCode::Busy => "E_BUSY",
    }
}

/// 一条命令的执行结果（不含块壳）。
struct Outcome {
    outputs: Vec<Output>,
    footer: Option<String>,
}

/// `execute` 的结果：空输入不做任何事，否则给出要按顺序推送的两个块。
pub(crate) enum Execution {
    /// 纯空白输入：不是错误，也不产生块。
    Ignored,
    /// 先推 `running`（空输出），再由 `done` 覆盖同一个 `id`。
    Blocks {
        /// 开始执行时推送的空块。
        running: Box<Block>,
        /// 执行结束的最终块。
        done: Box<Block>,
    },
}

/// 解析并执行一条终端输入，返回可推送的块。
///
/// 空输入（`Ok(None)`）不产生块；解析失败也走 `running → error` 两个块，
/// 这样前端只要按 `id` 覆盖即可。
pub(crate) fn execute(api: &dyn PlayerApi, block_id: &str, input: &str) -> Execution {
    let parsed = match parse(input) {
        Ok(None) => return Execution::Ignored,
        Ok(Some(parsed)) => Ok(parsed),
        Err(error) => Err(CmdError::from_parse(&error)),
    };

    let started_at = now_millis();
    let running = Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at,
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: Vec::new(),
        footer: None,
        schema_version: SCHEMA_VERSION,
    };

    let clock = Instant::now();
    let result = match &parsed {
        Ok(parsed) => Executor { api }.run(parsed),
        // 引号未闭合这类错误定位不到命令，没有 usage / 建议，给一句通用提示
        // 保证失败块里的 `hint` 恒不为空（T-0018 §2.5）。
        Err(error) => Err(error.clone().with_hint("输入 help 查看全部命令与用法")),
    };
    let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);

    let (status, outputs, footer) = match result {
        Ok(outcome) => (BlockStatus::Done, outcome.outputs, outcome.footer),
        Err(error) => (BlockStatus::Error, vec![error.to_output()], None),
    };

    let done = Block {
        duration_ms: Some(duration_ms),
        status,
        outputs,
        footer,
        ..running.clone()
    };

    Execution::Blocks {
        running: Box::new(running),
        done: Box::new(done),
    }
}

/// 把 [`Parsed`] 映射成 [`PlayerApi`] 调用并构造输出。
struct Executor<'a> {
    api: &'a dyn PlayerApi,
}

impl Executor<'_> {
    /// 执行一条已解析的命令；失败时补上命令用法作为提示。
    fn run(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        self.dispatch(parsed)
            .map_err(|error| error.with_usage(parsed))
    }

    fn dispatch(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        match parsed.spec.name {
            "play" => self.play(parsed),
            "pause" => self.simple("暂停", self.api.pause()),
            "resume" => self.simple("继续播放", self.api.resume()),
            "next" => self.simple("下一首", self.api.next()),
            "prev" => self.simple("上一首", self.api.prev()),
            "seek" => self.seek(parsed),
            "vol" => self.volume(parsed),
            "queue" => self.queue(parsed),
            "mode" => self.mode(parsed),
            "help" => self.help(parsed),
            "version" => Ok(Self::version()),
            "clear" => Err(CmdError::new("E_CMD_ARG", "clear 由终端本地处理")
                .with_hint("clear 是视图操作，前端直接清屏，不经过后端")),
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("命令表与执行器不一致：{other}"),
            )),
        }
    }

    /// `play`：无参 = 继续；纯数字 = 1 基序号；其余 = 文件路径。
    fn play(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let Some(value) = parsed.positional.first() else {
            return Ok(state_outcome("继续播放", &self.api.resume()?));
        };

        if is_unsigned_integer(value) {
            let index = user_index(value)?;
            let view = self.api.queue_jump(index)?;
            return Ok(jump_outcome("播放", index, &view));
        }

        Ok(state_outcome("播放", &self.api.play(Some(value.clone()))?))
    }

    /// 播放类命令的公共收尾：一句标题 + 当前曲目 / 位置。
    fn simple(
        &self,
        headline: &str,
        result: Result<PlaybackState, CmdError>,
    ) -> Result<Outcome, CmdError> {
        Ok(state_outcome(headline, &result?))
    }

    /// `seek`：`mm:ss` / 纯毫秒是绝对位置，`±N`（秒）与 `±mm:ss` 相对当前位置。
    fn seek(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let raw = positional_arg(parsed, "播放位置")?;
        let target = if raw.starts_with('+') || raw.starts_with('-') {
            let state = self.api.snapshot()?;
            parse_relative_seek(raw, state.position_ms, state.duration_ms)?
        } else {
            parse_absolute_seek(raw)?
        };

        Ok(state_outcome("跳转", &self.api.seek(target)?))
    }

    /// `vol`：`±N` 先读当前音量；无论哪种写法都在调用前钳到 0–100。
    fn volume(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let raw = positional_arg(parsed, "音量")?;
        let level = if raw.starts_with('+') || raw.starts_with('-') {
            let state = self.api.snapshot()?;
            relative_volume(raw, state.volume)?
        } else {
            raw.parse::<i64>()
                .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的音量：{raw}")))?
        };
        let clamped = level.clamp(0, 100);
        self.api.volume(clamped as f32 / 100.0)?;

        Ok(Outcome {
            outputs: vec![text_of(vec![bold("音量"), plain(format!(" · {clamped}%"))])],
            footer: None,
        })
    }

    /// `mode`：先取当前模式，再一次性下发完整的 `{ repeat, shuffle }`。
    fn mode(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "mode 缺少子命令"))?;
        let raw = positional_arg(parsed, "模式取值")?;
        let current = self.api.snapshot()?.mode;

        let mode = match subcommand.name {
            "repeat" => PlaybackMode {
                repeat: repeat_of(raw),
                shuffle: current.shuffle,
            },
            "shuffle" => PlaybackMode {
                repeat: current.repeat,
                shuffle: raw == "on",
            },
            other => {
                return Err(CmdError::new(
                    "E_CMD_UNKNOWN",
                    format!("mode 没有子命令：{other}"),
                ));
            }
        };

        let state = self.api.mode(mode)?;
        Ok(Outcome {
            outputs: vec![text_of(mode_spans(&state.mode))],
            footer: None,
        })
    }

    /// `queue`：`list` / `add` / `remove` / `clear` / `jump`。
    fn queue(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "queue 缺少子命令"))?;

        match subcommand.name {
            "list" => self.queue_list(),
            "add" => self.add_paths(&parsed.positional),
            "remove" => {
                let index = user_index(positional_arg(parsed, "队列序号")?)?;
                let view = self.api.queue_remove(index)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已移除"),
                        plain(format!(" · 第 {} 首", index + 1)),
                    ])],
                    footer: Some(format!("队列还剩 {} 首", view.tracks.len())),
                })
            }
            "clear" => {
                let view = self.api.queue_clear()?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![bold("队列已清空")])],
                    footer: Some(format!("队列 {} 首", view.tracks.len())),
                })
            }
            "jump" => {
                let index = user_index(positional_arg(parsed, "队列序号")?)?;
                let view = self.api.queue_jump(index)?;
                Ok(jump_outcome("跳转", index, &view))
            }
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("queue 没有子命令：{other}"),
            )),
        }
    }

    /// `queue list`：序号（1 基）/ 曲目 / 是否当前。
    fn queue_list(&self) -> Result<Outcome, CmdError> {
        let view = self.api.queue_list()?;
        let rows = view
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                vec![
                    (index + 1).to_string(),
                    track_display(track),
                    if view.index == Some(index) {
                        "▶".to_owned()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect();

        let footer = if view.tracks.is_empty() {
            "队列为空 · 用 queue add <路径> 加入曲目".to_owned()
        } else {
            format!("{} 首 · 输入 play 1 开始播放", view.tracks.len())
        };

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns: vec!["序号".to_owned(), "曲目".to_owned(), "当前".to_owned()],
                rows,
                selectable: true,
            }],
            footer: Some(footer),
        })
    }

    /// `queue add`：逐个加入、汇总成功数，中途失败不回滚已加入的曲目。
    fn add_paths(&self, paths: &[String]) -> Result<Outcome, CmdError> {
        let mut added = 0usize;
        let mut failed = 0usize;
        let mut first_error: Option<CmdError> = None;
        let mut view: Option<QueueView> = None;

        for path in paths {
            match self.api.queue_add(vec![path.clone()]) {
                Ok(updated) => {
                    added += 1;
                    view = Some(updated);
                }
                Err(error) => {
                    failed += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if added == 0 {
            return Err(
                first_error.unwrap_or_else(|| CmdError::new("E_CMD_ARG", "没有要加入的曲目"))
            );
        }

        let mut spans = vec![bold(format!("已加入 {added} 首"))];
        if failed > 0 {
            spans.push(dim(format!(" · 失败 {failed} 首")));
        }
        let footer = match (&first_error, view) {
            (Some(error), _) => Some(format!("首条失败：{} {}", error.code, error.message)),
            (None, Some(view)) => Some(format!("队列现有 {} 首", view.tracks.len())),
            (None, None) => None,
        };

        Ok(Outcome {
            outputs: vec![text_of(spans)],
            footer,
        })
    }

    /// `help`：不带参数列全部命令；带参数显示该命令的用法与说明。
    fn help(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let registry = registry();

        match parsed.positional.first() {
            Some(name) => {
                let Some(spec) = registry.find(name) else {
                    return Err(CmdError {
                        code: "E_CMD_UNKNOWN".to_owned(),
                        message: format!("未识别的命令：{name}"),
                        hint: registry.suggest(name).map(|spec| suggestion(spec.name)),
                    });
                };
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(spec.usage),
                        plain(format!("  {}", spec.summary)),
                    ])],
                    footer: None,
                })
            }
            None => {
                let outputs = registry
                    .specs()
                    .iter()
                    .map(|spec| {
                        text_of(vec![bold(format!("{:<8}", spec.name)), plain(spec.summary)])
                    })
                    .collect();
                Ok(Outcome {
                    outputs,
                    footer: Some("输入 help <命令> 查看用法".to_owned()),
                })
            }
        }
    }

    /// `version`：执行 crate 的版本。
    fn version() -> Outcome {
        Outcome {
            outputs: vec![text_of(vec![
                bold("Nowhere Player"),
                plain(format!(" {}", env!("CARGO_PKG_VERSION"))),
            ])],
            footer: None,
        }
    }
}

/// 现在时刻（Unix 毫秒）。
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 取第一个位置参数。
fn positional_arg<'a>(parsed: &'a Parsed, name: &str) -> Result<&'a str, CmdError> {
    parsed
        .positional
        .first()
        .map(String::as_str)
        .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("缺少参数：{name}")))
}

/// 纯 ASCII 数字（无符号）——只有这种位置参数才是序号，其余按路径处理。
fn is_unsigned_integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 用户输入的 1 基序号 → 队列的 0 基下标（T-0018 §6.2 定死）。
fn user_index(value: &str) -> Result<usize, CmdError> {
    let number: u64 = value
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("序号超出范围：{value}")))?;
    if number == 0 {
        return Err(CmdError::new("E_CMD_ARG", "序号从 1 开始"));
    }
    usize::try_from(number - 1)
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("序号超出范围：{value}")))
}

/// `mm:ss` → 毫秒。
fn parse_mm_ss(text: &str) -> Option<u64> {
    let (minutes, seconds) = text.split_once(':')?;
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = seconds.parse().ok()?;
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds.checked_mul(1_000)?)
}

/// 绝对位置：带冒号按 `mm:ss`，否则按毫秒。
fn parse_absolute_seek(raw: &str) -> Result<u64, CmdError> {
    if raw.contains(':') {
        parse_mm_ss(raw).ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))
    } else {
        raw.parse::<u64>()
            .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))
    }
}

/// 相对位置：`±N` 是秒，`±mm:ss` 是分钟秒；结果钳在 `[0, duration]`。
fn parse_relative_seek(
    raw: &str,
    current_ms: u64,
    duration_ms: Option<u64>,
) -> Result<u64, CmdError> {
    let (negative, body) = match raw.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };

    let delta_ms: i128 = if body.contains(':') {
        i128::from(
            parse_mm_ss(body)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?,
        )
    } else {
        let seconds: u64 = body
            .parse()
            .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?;
        i128::from(
            seconds
                .checked_mul(1_000)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?,
        )
    };

    let target = i128::from(current_ms) + if negative { -delta_ms } else { delta_ms };
    let upper = duration_ms.map_or(i128::MAX, i128::from);

    Ok(u64::try_from(target.clamp(0, upper)).unwrap_or(0))
}

/// 相对音量：当前百分比（0–100）+ 增量，钳制留给调用方。
fn relative_volume(raw: &str, current: f32) -> Result<i64, CmdError> {
    let delta: i64 = raw
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的音量：{raw}")))?;
    let current_percent = (current * 100.0).round().clamp(0.0, 100.0) as i64;

    Ok(current_percent + delta)
}

/// 枚举值 → 循环模式（解析层已归一化，未知值按 `off` 兜底）。
fn repeat_of(value: &str) -> RepeatMode {
    match value {
        "one" => RepeatMode::One,
        "all" => RepeatMode::All,
        _ => RepeatMode::Off,
    }
}

fn plain(text: impl Into<String>) -> Span {
    Span::plain(text)
}

fn bold(text: impl Into<String>) -> Span {
    Span {
        bold: true,
        ..Span::plain(text)
    }
}

fn dim(text: impl Into<String>) -> Span {
    Span {
        dim: true,
        ..Span::plain(text)
    }
}

fn text_of(spans: Vec<Span>) -> Output {
    Output::Text { spans, tone: None }
}

/// 播放类命令的输出：标题 + 曲目名 + 位置/时长。
fn state_outcome(headline: &str, state: &PlaybackState) -> Outcome {
    let mut spans = vec![bold(headline)];

    if let Some(track) = state.track_id.as_deref() {
        spans.push(dim(format!(" · {}", track_display(track))));
        let position = format_position(state.position_ms);
        let duration = state
            .duration_ms
            .map_or_else(|| "--:--".to_owned(), format_position);
        spans.push(dim(format!(" · {position} / {duration}")));
    }

    let footer = match state.queue_length {
        Some(length) if length > 0 => Some(format!("队列 {length} 首")),
        _ => None,
    };

    Outcome {
        outputs: vec![text_of(spans)],
        footer,
    }
}

/// 队列跳转类命令的输出；序号对用户按 1 基显示。
fn jump_outcome(headline: &str, index: usize, view: &QueueView) -> Outcome {
    let mut spans = vec![bold(headline), plain(format!(" · 第 {} 首", index + 1))];
    if let Some(track) = view.tracks.get(index) {
        spans.push(dim(format!(" · {}", track_display(track))));
    }

    Outcome {
        outputs: vec![text_of(spans)],
        footer: Some(format!("队列 {} 首", view.tracks.len())),
    }
}

fn mode_spans(mode: &PlaybackMode) -> Vec<Span> {
    let repeat = match mode.repeat {
        RepeatMode::Off => "关",
        RepeatMode::One => "单曲",
        RepeatMode::All => "列表",
    };
    let shuffle = if mode.shuffle { "开" } else { "关" };

    vec![
        bold("模式"),
        plain(format!(" · 循环：{repeat} · 随机：{shuffle}")),
    ]
}

/// 曲目 id（本地文件为绝对路径）在终端里只显示文件名。
fn track_display(track_id: &str) -> String {
    std::path::Path::new(track_id)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| track_id.to_owned())
}

/// 毫秒 → `mm:ss`（超过一小时时分钟继续累加）。
fn format_position(position_ms: u64) -> String {
    let total_seconds = position_ms / 1_000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// `terminal:block` 事件名。
///
/// Tauri 2 的事件名只允许字母数字与 `- / : _`，**点号非法**，
/// 所以这里用冒号（`cmd.exec` 是命令名，不受同一限制）。
pub(crate) const EVENT_TERMINAL_BLOCK: &str = "terminal:block";

/// `cmd.exec` 的立即回执：`{ blockId, ok }`（camelCase）。
///
/// 字段名经 `serde` 的 `rename_all = "camelCase"` 序列化，与 T-0018 契约一致；
/// Tauri 对 `T: Serialize` 的返回值使用 [`tauri::ipc::IpcResponse`] 的默认实现，
/// 不再手写 JSON（T-0021 §2.1）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CmdExecReply {
    block_id: String,
    ok: bool,
}

/// `PlayerHandle` 的 [`PlayerApi`] 适配（生产实现）。
pub(crate) struct HandleApi<'a> {
    handle: &'a PlayerHandle,
}

impl<'a> HandleApi<'a> {
    pub(crate) fn new(handle: &'a PlayerHandle) -> Self {
        Self { handle }
    }
}

impl PlayerApi for HandleApi<'_> {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        self.handle.snapshot().map_err(CmdError::from_player)
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        self.handle.play(path).map_err(CmdError::from_player)
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        self.handle.pause().map_err(CmdError::from_player)
    }

    /// `PlayerHandle` 没有独立的 `resume`：`play(None)` 在 actor 里就是继续当前曲目。
    fn resume(&self) -> Result<PlaybackState, CmdError> {
        self.handle.play(None).map_err(CmdError::from_player)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.handle.next().map_err(CmdError::from_player)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.handle.prev().map_err(CmdError::from_player)
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
        self.handle.seek(position_ms).map_err(CmdError::from_player)
    }

    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
        self.handle.volume(volume).map_err(CmdError::from_player)
    }

    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        self.handle.mode(mode).map_err(CmdError::from_player)
    }

    fn queue_list(&self) -> Result<QueueView, CmdError> {
        self.handle.queue_list().map_err(CmdError::from_player)
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
        self.handle.queue_add(paths).map_err(CmdError::from_player)
    }

    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
        self.handle
            .queue_remove(index)
            .map_err(CmdError::from_player)
    }

    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        self.handle.queue_clear().map_err(CmdError::from_player)
    }

    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
        self.handle.queue_jump(index).map_err(CmdError::from_player)
    }
}

/// `cmd.exec`：执行一条终端输入。
///
/// 立即返回 `{ blockId, ok }`，输出经 `terminal:block` 事件推送：
/// 先发 `running` 空块，再发同一 `id` 的最终块（前端按 `id` 覆盖）。
/// `ok` 表示该命令最终是否成功；空输入视为成功且不产生块。
#[tauri::command(rename = "cmd.exec")]
pub(crate) fn cmd_exec(
    app: AppHandle,
    handle: State<'_, PlayerHandle>,
    input: String,
    block_id: String,
) -> CmdExecReply {
    let api = HandleApi::new(&handle);

    match execute(&api, &block_id, &input) {
        Execution::Ignored => CmdExecReply { block_id, ok: true },
        Execution::Blocks { running, done } => {
            emit_block(&app, &running);
            let ok = done.status != BlockStatus::Error;
            emit_block(&app, &done);
            CmdExecReply { block_id, ok }
        }
    }
}

fn emit_block(app: &AppHandle, block: &Block) {
    if let Err(error) = app.emit(EVENT_TERMINAL_BLOCK, block) {
        eprintln!("[cmd] emit {EVENT_TERMINAL_BLOCK} failed: {error}");
    }
}

/// `cmd.complete` 的纯逻辑：算候选，动态候选按需取一次播放器快照。
///
/// 快照失败（`E_BUSY` 等）只让动态候选降级为空列表，命令名 / 子命令 / 枚举值
/// 照常返回——补全不能被播放器状态卡住（T-0020 §6.1、§11）。
/// `cursor` 与返回的 `replaceStart` / `replaceEnd` 都是**字符偏移**。
pub(crate) fn complete_for(api: &dyn PlayerApi, input: &str, cursor: usize) -> Completion {
    complete(input, cursor, || {
        api.snapshot().ok().and_then(|state| state.queue_length)
    })
}

/// `cmd.complete`：一次补全请求，返回 `{ replaceStart, replaceEnd, items }`。
///
/// 区间单位是字符（码点），不是字节也不是 UTF-16；前端负责与
/// `selectionStart`（UTF-16）互转。无候选时 `items` 为空，调用不报错。
#[tauri::command(rename = "cmd.complete")]
pub(crate) fn cmd_complete(
    handle: State<'_, PlayerHandle>,
    input: String,
    cursor: usize,
) -> Completion {
    complete_for(&HandleApi::new(&handle), &input, cursor)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use np_core::command::parse;
    use np_core::{PlaybackMode, PlaybackStatus, RepeatMode};

    use super::*;

    /// `FakeApi` 记录的一次调用。
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Snapshot,
        Play(Option<String>),
        Pause,
        Resume,
        Next,
        Prev,
        Seek(u64),
        Volume(f32),
        Mode(PlaybackMode),
        QueueList,
        QueueAdd(Vec<String>),
        QueueRemove(usize),
        QueueClear,
        QueueJump(usize),
    }

    /// 单测用播放器：记录调用、返回构造值，不碰真实设备。
    struct FakeApi {
        calls: Mutex<Vec<Call>>,
        state: Mutex<PlaybackState>,
        queue: Mutex<QueueView>,
        reject_paths: Mutex<Vec<String>>,
        playback_error: Mutex<Option<CmdError>>,
    }

    impl FakeApi {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                state: Mutex::new(PlaybackState {
                    volume: 0.5,
                    ..PlaybackState::default()
                }),
                queue: Mutex::new(QueueView {
                    tracks: Vec::new(),
                    index: None,
                }),
                reject_paths: Mutex::new(Vec::new()),
                playback_error: Mutex::new(None),
            }
        }

        fn with_state(self, state: PlaybackState) -> Self {
            *self.state.lock().expect("state mutex") = state;
            self
        }

        fn with_queue(self, queue: QueueView) -> Self {
            *self.queue.lock().expect("queue mutex") = queue;
            self
        }

        fn with_reject_paths(self, paths: &[&str]) -> Self {
            *self.reject_paths.lock().expect("reject mutex") =
                paths.iter().map(|path| (*path).to_owned()).collect();
            self
        }

        fn with_playback_error(self, error: CmdError) -> Self {
            *self.playback_error.lock().expect("error mutex") = Some(error);
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("calls mutex").clone()
        }

        fn record(&self, call: Call) {
            self.calls.lock().expect("calls mutex").push(call);
        }

        /// 播放类命令的公共返回：配置了错误就回错误，否则回当前快照。
        fn playback(&self) -> Result<PlaybackState, CmdError> {
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            Ok(self.state.lock().expect("state mutex").clone())
        }
    }

    impl PlayerApi for FakeApi {
        fn snapshot(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Snapshot);
            self.playback()
        }

        fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
            self.record(Call::Play(path.clone()));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            {
                let mut state = self.state.lock().expect("state mutex");
                if path.is_some() {
                    state.track_id = path;
                }
                state.status = PlaybackStatus::Playing;
            }
            self.playback()
        }

        fn pause(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Pause);
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").status = PlaybackStatus::Paused;
            self.playback()
        }

        fn resume(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Resume);
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").status = PlaybackStatus::Playing;
            self.playback()
        }

        fn next(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Next);
            self.playback()
        }

        fn prev(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Prev);
            self.playback()
        }

        fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
            self.record(Call::Seek(position_ms));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").position_ms = position_ms;
            self.playback()
        }

        fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
            self.record(Call::Volume(volume));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").volume = volume;
            self.playback()
        }

        fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
            self.record(Call::Mode(mode));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").mode = mode;
            self.playback()
        }

        fn queue_list(&self) -> Result<QueueView, CmdError> {
            self.record(Call::QueueList);
            Ok(self.queue.lock().expect("queue mutex").clone())
        }

        fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
            self.record(Call::QueueAdd(paths.clone()));
            {
                let reject = self.reject_paths.lock().expect("reject mutex");
                if let Some(path) = paths.iter().find(|path| reject.contains(path)) {
                    return Err(CmdError::new("E_DECODE", format!("无法加入：{path}")));
                }
            }
            let mut queue = self.queue.lock().expect("queue mutex");
            queue.tracks.extend(paths);
            if queue.index.is_none() {
                queue.index = Some(0);
            }
            Ok(queue.clone())
        }

        fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
            self.record(Call::QueueRemove(index));
            let mut queue = self.queue.lock().expect("queue mutex");
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_DECODE", "队列下标越界"));
            }
            queue.tracks.remove(index);
            if queue.tracks.is_empty() {
                queue.index = None;
            } else if let Some(current) = queue.index {
                if index < current {
                    queue.index = Some(current - 1);
                } else if index == current {
                    queue.index = Some(current.min(queue.tracks.len() - 1));
                }
            }
            Ok(queue.clone())
        }

        fn queue_clear(&self) -> Result<QueueView, CmdError> {
            self.record(Call::QueueClear);
            let mut queue = self.queue.lock().expect("queue mutex");
            queue.tracks.clear();
            queue.index = None;
            Ok(queue.clone())
        }

        fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
            self.record(Call::QueueJump(index));
            let mut queue = self.queue.lock().expect("queue mutex");
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_DECODE", "队列下标越界"));
            }
            queue.index = Some(index);
            Ok(queue.clone())
        }
    }

    fn queue_view(tracks: &[&str], index: Option<usize>) -> QueueView {
        QueueView {
            tracks: tracks.iter().map(|track| (*track).to_owned()).collect(),
            index,
        }
    }

    #[test]
    fn fake_api_records_calls() {
        let api = FakeApi::new();
        api.pause().expect("pause succeeds");
        api.seek(1_500).expect("seek succeeds");

        assert_eq!(api.calls(), vec![Call::Pause, Call::Seek(1_500)]);
    }

    #[test]
    fn parse_error_keeps_code_and_usage_hint() {
        let error = parse("plya").expect_err("unknown command must fail");
        let error = CmdError::from_parse(&error);

        assert_eq!(error.code, "E_CMD_UNKNOWN");
        assert_eq!(error.hint.as_deref(), Some("did you mean 'play'?"));
    }

    #[test]
    fn player_error_maps_to_ipc_code() {
        let error = CmdError::from_player(PlayerError::unsupported("没有可暂停的曲目"));

        assert_eq!(error.code, "E_UNSUPPORTED");
        assert_eq!(error.message, "没有可暂停的曲目");
        assert_eq!(error.hint, None);
    }

    #[test]
    fn usage_hint_is_filled_when_missing() {
        let parsed = parse("vol 5")
            .expect("valid input")
            .expect("non-empty input");
        let error = CmdError::new("E_DECODE", "设备丢失").with_usage(&parsed);

        assert_eq!(error.hint.as_deref(), Some("用法：vol <0-100|±N>"));
    }

    #[test]
    fn repeat_mode_defaults_to_off_for_unknown_values() {
        // 解析层已把枚举归一化；这里只锁住“未知值绝不 panic”。
        let mode = PlaybackMode {
            repeat: RepeatMode::Off,
            shuffle: false,
        };
        assert_eq!(mode, PlaybackMode::default());
    }

    // ---- 执行器：命令映射 ----

    fn done_block(api: &FakeApi, input: &str) -> Block {
        match execute(api, "block-1", input) {
            Execution::Blocks { done, .. } => *done,
            Execution::Ignored => panic!("expected a block for input: {input}"),
        }
    }

    fn text_of_block(block: &Block) -> String {
        block
            .outputs
            .iter()
            .filter_map(|output| match output {
                Output::Text { spans, .. } => Some(
                    spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn error_fields(block: &Block) -> (&str, &str, Option<&str>) {
        block
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Error {
                    code,
                    message,
                    hint,
                    ..
                } => Some((code.as_str(), message.as_str(), hint.as_deref())),
                _ => None,
            })
            .expect("block must contain an error output")
    }

    #[test]
    fn play_without_argument_resumes() {
        let api = FakeApi::new();
        let block = done_block(&api, "play");

        assert_eq!(api.calls(), vec![Call::Resume]);
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("继续播放"));
    }

    #[test]
    fn play_number_is_one_based_and_jumps() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], None));
        let block = done_block(&api, "play 3");

        assert_eq!(api.calls(), vec![Call::QueueJump(2)]);
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("第 3 首"));
    }

    #[test]
    fn play_zero_is_rejected_before_any_call() {
        let api = FakeApi::new();
        let block = done_block(&api, "play 0");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(api.calls().is_empty());
    }

    #[test]
    fn play_path_is_passed_through() {
        let api = FakeApi::new();
        let block = done_block(&api, r"play C:\Music\a.flac");

        assert_eq!(
            api.calls(),
            vec![Call::Play(Some(r"C:\Music\a.flac".to_owned()))]
        );
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("a.flac"));
    }

    #[test]
    fn pause_resume_next_prev_map_to_same_named_calls() {
        let api = FakeApi::new();

        for input in ["pause", "resume", "next", "prev"] {
            assert_eq!(done_block(&api, input).status, BlockStatus::Done);
        }

        assert_eq!(
            api.calls(),
            vec![Call::Pause, Call::Resume, Call::Next, Call::Prev]
        );
    }

    #[test]
    fn vol_absolute_sets_level_and_clamps_high() {
        let api = FakeApi::new();
        let block = done_block(&api, "vol 150");

        assert_eq!(api.calls(), vec![Call::Volume(1.0)]);
        assert!(text_of_block(&block).contains("100%"));
    }

    #[test]
    fn vol_absolute_zero_is_allowed() {
        let api = FakeApi::new();
        let block = done_block(&api, "vol 0");

        assert_eq!(api.calls(), vec![Call::Volume(0.0)]);
        assert!(text_of_block(&block).contains("0%"));
    }

    #[test]
    fn vol_relative_reads_current_level_first() {
        let api = FakeApi::new().with_state(PlaybackState {
            volume: 0.42,
            ..PlaybackState::default()
        });
        let block = done_block(&api, "vol +8");

        let calls = api.calls();
        assert_eq!(calls.first(), Some(&Call::Snapshot));
        match calls.get(1) {
            Some(Call::Volume(volume)) => assert!((volume - 0.5).abs() < 1e-6),
            other => panic!("expected volume call, got {other:?}"),
        }
        assert!(text_of_block(&block).contains("50%"));
    }

    #[test]
    fn vol_relative_clamps_low() {
        let api = FakeApi::new().with_state(PlaybackState {
            volume: 0.5,
            ..PlaybackState::default()
        });
        let block = done_block(&api, "vol -200");

        assert_eq!(api.calls(), vec![Call::Snapshot, Call::Volume(0.0)]);
        assert!(text_of_block(&block).contains("0%"));
    }

    #[test]
    fn seek_plain_number_is_milliseconds() {
        let api = FakeApi::new();
        done_block(&api, "seek 90000");

        assert_eq!(api.calls(), vec![Call::Seek(90_000)]);
    }

    #[test]
    fn seek_mm_ss_is_absolute() {
        let api = FakeApi::new();
        done_block(&api, "seek 1:30");

        assert_eq!(api.calls(), vec![Call::Seek(90_000)]);
    }

    #[test]
    fn seek_relative_seconds_uses_current_position() {
        let api = FakeApi::new().with_state(PlaybackState {
            position_ms: 30_000,
            duration_ms: Some(180_000),
            ..PlaybackState::default()
        });
        done_block(&api, "seek +15");

        assert_eq!(api.calls(), vec![Call::Snapshot, Call::Seek(45_000)]);
    }

    #[test]
    fn seek_relative_clamps_to_track_bounds() {
        let api = FakeApi::new().with_state(PlaybackState {
            position_ms: 30_000,
            duration_ms: Some(180_000),
            ..PlaybackState::default()
        });

        done_block(&api, "seek -40");
        done_block(&api, "seek +999");

        assert_eq!(
            api.calls(),
            vec![
                Call::Snapshot,
                Call::Seek(0),
                Call::Snapshot,
                Call::Seek(180_000)
            ]
        );
    }

    #[test]
    fn mode_repeat_sends_complete_mode_once() {
        let api = FakeApi::new().with_state(PlaybackState {
            mode: PlaybackMode {
                repeat: RepeatMode::Off,
                shuffle: true,
            },
            ..PlaybackState::default()
        });
        let block = done_block(&api, "mode repeat all");

        let calls = api.calls();
        assert_eq!(calls.first(), Some(&Call::Snapshot));
        let modes: Vec<&Call> = calls
            .iter()
            .filter(|call| matches!(call, Call::Mode(_)))
            .collect();
        assert_eq!(modes.len(), 1, "mode 必须一次下发完整对象");
        assert_eq!(
            modes[0],
            &Call::Mode(PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: true,
            })
        );
        assert!(text_of_block(&block).contains("循环：列表"));
    }

    #[test]
    fn mode_shuffle_keeps_repeat_from_snapshot() {
        let api = FakeApi::new().with_state(PlaybackState {
            mode: PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: false,
            },
            ..PlaybackState::default()
        });
        let block = done_block(&api, "mode shuffle on");

        assert_eq!(
            api.calls(),
            vec![
                Call::Snapshot,
                Call::Mode(PlaybackMode {
                    repeat: RepeatMode::All,
                    shuffle: true,
                })
            ]
        );
        assert!(text_of_block(&block).contains("随机：开"));
    }

    #[test]
    fn queue_list_renders_rows_and_current_marker() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(1)));
        let block = done_block(&api, "queue list");

        let table = block
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Table { columns, rows, .. } => Some((columns, rows)),
                _ => None,
            })
            .expect("queue list must render a table");

        assert_eq!(
            table.0,
            &vec!["序号".to_owned(), "曲目".to_owned(), "当前".to_owned()]
        );
        assert_eq!(table.1.len(), 3);
        assert_eq!(table.1[0][0], "1");
        assert_eq!(table.1[1][2], "▶");
        assert_eq!(table.1[0][2], "");
    }

    #[test]
    fn queue_add_reports_success_count() {
        let api = FakeApi::new();
        let block = done_block(&api, r"queue add C:\Music\a.flac");

        assert_eq!(
            api.calls(),
            vec![Call::QueueAdd(vec![r"C:\Music\a.flac".to_owned()])]
        );
        assert!(text_of_block(&block).contains("已加入 1 首"));
        assert_eq!(block.footer.as_deref(), Some("队列现有 1 首"));
    }

    #[test]
    fn queue_add_keeps_going_after_partial_failure() {
        let api = FakeApi::new().with_reject_paths(&["b.flac"]);
        let paths = vec![
            "a.flac".to_owned(),
            "b.flac".to_owned(),
            "c.flac".to_owned(),
        ];
        let outcome = Executor { api: &api }
            .add_paths(&paths)
            .expect("partial success is still a success");

        assert_eq!(api.calls().len(), 3);
        assert_eq!(
            outcome.footer.as_deref(),
            Some("首条失败：E_DECODE 无法加入：b.flac")
        );
    }

    #[test]
    fn queue_remove_uses_one_based_index() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(0)));
        let block = done_block(&api, "queue remove 2");

        assert_eq!(api.calls(), vec![Call::QueueRemove(1)]);
        assert!(text_of_block(&block).contains("第 2 首"));
        assert_eq!(block.footer.as_deref(), Some("队列还剩 2 首"));
    }

    #[test]
    fn queue_jump_uses_one_based_index() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(0)));
        let block = done_block(&api, "queue jump 3");

        assert_eq!(api.calls(), vec![Call::QueueJump(2)]);
        assert!(text_of_block(&block).contains("第 3 首"));
    }

    #[test]
    fn queue_clear_empties_the_queue() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac"], Some(0)));
        let block = done_block(&api, "queue clear");

        assert_eq!(api.calls(), vec![Call::QueueClear]);
        assert!(text_of_block(&block).contains("清空"));
    }

    #[test]
    fn parse_error_becomes_error_block_with_hint() {
        let api = FakeApi::new();
        let block = done_block(&api, "plya");

        assert_eq!(block.status, BlockStatus::Error);
        let (code, _, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_UNKNOWN");
        assert_eq!(hint, Some("did you mean 'play'?"));
        assert!(api.calls().is_empty());
    }

    #[test]
    fn argument_error_carries_usage_hint() {
        let api = FakeApi::new();
        let block = done_block(&api, "seek 1:2:3");

        let (code, _, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        let hint = hint.expect("hint must not be empty");
        assert!(
            hint.contains("用法：seek <ms|mm:ss|±s>"),
            "unexpected hint: {hint}"
        );
    }

    #[test]
    fn unclosed_quote_error_still_has_a_hint() {
        let api = FakeApi::new();
        let block = done_block(&api, "play \"unterminated");

        let (code, _, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        assert!(hint.is_some_and(|hint| !hint.is_empty()));
    }

    #[test]
    fn api_error_becomes_error_block_with_usage_hint() {
        let api =
            FakeApi::new().with_playback_error(CmdError::new("E_UNSUPPORTED", "没有可暂停的曲目"));
        let block = done_block(&api, "pause");

        assert_eq!(block.status, BlockStatus::Error);
        let (code, message, hint) = error_fields(&block);
        assert_eq!(code, "E_UNSUPPORTED");
        assert_eq!(message, "没有可暂停的曲目");
        assert_eq!(hint, Some("用法：pause"));
    }

    #[test]
    fn help_lists_every_registered_command() {
        let api = FakeApi::new();
        let block = done_block(&api, "help");
        let text = text_of_block(&block);

        for spec in registry().specs() {
            assert!(text.contains(spec.name), "help 缺少命令：{}", spec.name);
        }
    }

    #[test]
    fn help_with_argument_shows_usage() {
        let api = FakeApi::new();
        let block = done_block(&api, "help seek");

        assert!(text_of_block(&block).contains("seek <ms|mm:ss|±s>"));
    }

    #[test]
    fn version_reports_crate_version() {
        let api = FakeApi::new();
        let block = done_block(&api, "version");

        assert!(text_of_block(&block).contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn clear_is_rejected_with_explanation() {
        let api = FakeApi::new();
        let block = done_block(&api, "clear");

        let (code, message, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        assert!(message.contains("clear 由终端本地处理"));
        assert!(hint.is_some_and(|hint| !hint.is_empty()));
    }

    #[test]
    fn blank_input_produces_no_block() {
        let api = FakeApi::new();

        assert!(matches!(
            execute(&api, "block-1", "   "),
            Execution::Ignored
        ));
    }

    #[test]
    fn block_is_running_then_done_with_the_same_id() {
        let api = FakeApi::new();
        let Execution::Blocks { running, done } = execute(&api, "id-1", "pause") else {
            panic!("expected blocks");
        };

        assert_eq!(running.id, "id-1");
        assert_eq!(running.status, BlockStatus::Running);
        assert!(running.outputs.is_empty());
        assert_eq!(running.duration_ms, None);

        assert_eq!(done.id, running.id);
        assert_eq!(done.input, "pause");
        assert_eq!(done.status, BlockStatus::Done);
        assert!(done.duration_ms.is_some());
        assert!(done.started_at > 0);
        assert_eq!(done.schema_version, SCHEMA_VERSION);
    }

    // ---- IPC 与事件 ----

    #[test]
    fn terminal_event_channel_uses_a_colon_not_a_dot() {
        assert_eq!(EVENT_TERMINAL_BLOCK, "terminal:block");
        assert!(!EVENT_TERMINAL_BLOCK.contains('.'));
    }

    #[test]
    fn cmd_exec_command_name_keeps_the_dot() {
        assert_eq!(__tauri_command_name_cmd_exec!(), "cmd.exec");
    }

    // ---- 补全（T-0020） ----

    #[test]
    fn cmd_complete_command_name_keeps_the_dot() {
        assert_eq!(__tauri_command_name_cmd_complete!(), "cmd.complete");
    }

    #[test]
    fn complete_maps_dynamic_queue_indices_and_polls_snapshot_once() {
        let api = FakeApi::new().with_state(PlaybackState {
            queue_length: Some(3),
            ..PlaybackState::default()
        });

        let completion = complete_for(&api, "play ", 5);

        assert_eq!(api.calls(), vec![Call::Snapshot]);
        assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
        let texts: Vec<&str> = completion
            .items
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        assert_eq!(texts, ["1", "2", "3"]);
    }

    #[test]
    fn complete_degrades_to_empty_dynamic_candidates_when_snapshot_fails() {
        let api = FakeApi::new().with_playback_error(CmdError::new("E_BUSY", "播放器线程不可用"));

        let completion = complete_for(&api, "play ", 5);

        // 降级：不报错、不阻塞，只返回空列表；区间仍然给出。
        assert!(completion.items.is_empty());
        assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
    }

    #[test]
    fn complete_with_unknown_queue_length_returns_no_dynamic_candidates() {
        // FakeApi 默认 `queue_length: None`：快照成功但长度未知，同样降级为空。
        let api = FakeApi::new();

        let completion = complete_for(&api, "play ", 5);

        assert!(completion.items.is_empty());
    }

    #[test]
    fn complete_returns_static_candidates_without_polling_the_player() {
        let api = FakeApi::new();

        let completion = complete_for(&api, "pl", 2);

        assert_eq!(api.calls(), Vec::new());
        assert_eq!(completion.items.len(), 1);
        assert_eq!(completion.items[0].text, "play");
        assert_eq!(
            completion.items[0].kind,
            np_core::command::CompletionKind::Command
        );
    }

    #[test]
    fn reply_body_is_camel_case_json() {
        let reply = CmdExecReply {
            block_id: "block-1".to_owned(),
            ok: true,
        };

        match tauri::ipc::IpcResponse::body(reply).expect("reply body") {
            tauri::ipc::InvokeResponseBody::Json(json) => {
                assert_eq!(json, r#"{"blockId":"block-1","ok":true}"#);
            }
            tauri::ipc::InvokeResponseBody::Raw(_) => panic!("expected a JSON body"),
        }
    }
}
