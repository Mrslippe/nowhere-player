//! 命令补全（T-0020 §6.1）。
//!
//! 纯函数：给定一行输入与光标位置，算出候选列表与**替换区间**。
//! 不执行命令、不碰 IPC、不访问播放器；动态候选（`play <n>`）通过调用方
//! 注入的 `queue_length` 闭包获取，拿不到时降级为空列表，绝不报错。
//!
//! # 替换区间单位：字符
//!
//! [`Completion::replace_start`] / [`Completion::replace_end`] 与 `cursor` 一样，
//! 都是 **Unicode 字符（码点）偏移**——不是字节偏移，也不是 UTF-16 偏移。
//! 前端 `selectionStart` 是 UTF-16，必须先转换；否则含中文/emoji 的输入会截错位置。
//!
//! # 匹配规则
//!
//! - 命令名 / 子命令 / 枚举值：大小写不敏感的前缀匹配；
//! - 前缀无命中时，按编辑距离 ≤ 2 给拼写建议（候选类型标为 `suggestion`）；
//! - `play` + 空 token + 队列长度已知：给 `1..=queueLength` 的动态序号；
//! - 其余位置（路径、整数、播放位置等）不做补全（§3 非目标）。

use serde::Serialize;

use super::registry::{
    ArgKind, ArgSpec, CommandSpec, Registry, SubcommandSpec, edit_distance, registry,
};

/// 候选的补全结果条数上限（弹层只显示前 8 条，见 T-0020 §6.3）。
pub const MAX_ITEMS: usize = 20;

/// 候选来源，前端据此显示类型标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompletionKind {
    /// 顶层命令名。
    Command,
    /// 子命令名。
    Subcommand,
    /// 枚举值（`mode repeat <off|one|all>`）。
    #[serde(rename = "enum")]
    EnumValue,
    /// 动态候选（队列序号）。
    Dynamic,
    /// 拼写建议（编辑距离 ≤ 2）；不是前缀命中。
    Suggestion,
}

/// 一条候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    /// 采纳后用来替换 `replaceStart..replaceEnd` 的文本（命令/子命令为规范小写写法）。
    pub text: String,
    /// 候选来源。
    pub kind: CompletionKind,
    /// 一行说明，可直接展示。
    pub summary: String,
}

/// 一次补全结果。
///
/// 区间对所有候选**相同**：替换的是光标所在的当前 token（不含两侧空白）。
/// 无候选时仍返回区间，前端据此知道该 token 在哪，只是不弹层、不改输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    /// 替换起点（字符偏移，0 基，含）。
    pub replace_start: usize,
    /// 替换终点（字符偏移，不含）；等于起点表示插入。
    pub replace_end: usize,
    /// 候选列表；空表示没有可补全项。
    pub items: Vec<CompletionItem>,
}

impl Completion {
    /// 无候选结果：保留区间，便于前端统一处理。
    fn empty(replace_start: usize, replace_end: usize) -> Self {
        Self {
            replace_start,
            replace_end,
            items: Vec::new(),
        }
    }
}

/// 算一次补全。
///
/// `queue_length` 只在命中动态候选（`play` + 空 token）时调用一次；
/// 返回 `None`（播放器快照失败 / 队列长度未知）时动态候选降级为空列表。
pub fn complete<F>(input: &str, cursor: usize, queue_length: F) -> Completion
where
    F: FnOnce() -> Option<usize>,
{
    complete_with(registry(), input, cursor, queue_length)
}

/// [`complete`] 的显式注册表版本（单测注入自定义命令表用）。
fn complete_with<F>(registry: &Registry, input: &str, cursor: usize, queue_length: F) -> Completion
where
    F: FnOnce() -> Option<usize>,
{
    let chars: Vec<char> = input.chars().collect();
    let cursor = cursor.min(chars.len());

    // 当前 token = 光标两侧到最近的空白为止；同一 token 中间的空白不存在，
    // 所以左右各扫一次即可。区间按字符计（见模块文档）。
    let start = chars[..cursor]
        .iter()
        .rposition(|character| character.is_whitespace())
        .map_or(0, |index| index + 1);
    let end = chars[cursor..]
        .iter()
        .position(|character| character.is_whitespace())
        .map_or(chars.len(), |offset| cursor + offset);
    let token: String = chars[start..end].iter().collect();

    // token 之前的词（命令 / 子命令已打完的部分），只按空白切分：
    // 补全只关心命令词，不做引号/转义的完整词法分析。
    let prefix: String = chars[..start].iter().collect();
    let words: Vec<&str> = prefix.split_whitespace().collect();

    let mut completion = Completion::empty(start, end);
    completion.items = match resolve(registry, &words, &token) {
        Plan::Empty => Vec::new(),
        Plan::Dynamic => dynamic_items(queue_length()),
        Plan::Entries { kind, entries } => match_items(kind, &entries, &token),
    };
    completion
}

/// 解析「这个位置该补什么」的中间结果。
///
/// 单独一层是为了让动态候选的闭包只在一个分支里被调用（`FnOnce`），
/// 其余位置（路径、整数）绝不触发播放器快照。
enum Plan {
    /// 该位置没有可补全项。
    Empty,
    /// 动态队列序号。
    Dynamic,
    /// 静态候选：文本 + 说明。
    Entries {
        kind: CompletionKind,
        entries: Vec<(String, String)>,
    },
}

fn resolve(registry: &Registry, words: &[&str], token: &str) -> Plan {
    let Some(head) = words.first() else {
        // 命令位置（含空输入）。
        return Plan::Entries {
            kind: CompletionKind::Command,
            entries: registry
                .specs()
                .iter()
                .map(|spec| (spec.name.to_owned(), spec.summary.to_owned()))
                .collect(),
        };
    };

    let Some(spec) = registry.find(head) else {
        // 未知命令：不猜参数，交给解析器报错。
        return Plan::Empty;
    };

    if spec.subcommands.is_empty() {
        // 没有子命令：`words.len() - 1` 就是下一个位置参数的序号。
        return arg_plan(spec, spec.args, words.len() - 1, token);
    }

    if words.len() == 1 {
        return Plan::Entries {
            kind: CompletionKind::Subcommand,
            entries: subcommand_entries(spec.subcommands),
        };
    }

    let Some(subcommand) = find_subcommand(spec, words[1]) else {
        return Plan::Empty;
    };
    arg_plan(spec, subcommand.args, words.len() - 2, token)
}

/// 位置参数补全：目前只有枚举值有静态候选，`play` 第 1 个参数是动态序号。
fn arg_plan(
    spec: &'static CommandSpec,
    args: &'static [ArgSpec],
    index: usize,
    token: &str,
) -> Plan {
    let Some(arg) = args.get(index) else {
        return Plan::Empty;
    };

    if let ArgKind::Enum(values) = arg.kind {
        return Plan::Entries {
            kind: CompletionKind::EnumValue,
            entries: values
                .iter()
                .map(|value| ((*value).to_owned(), format!("{} 的取值", arg.name)))
                .collect(),
        };
    }

    // 动态候选只在 `play` 后面还没有任何 token 时给出（`play `），
    // 此时光标紧跟空格；已经开始输入序号/路径时不再猜。
    if spec.name == "play" && index == 0 && token.is_empty() {
        return Plan::Dynamic;
    }

    Plan::Empty
}

fn subcommand_entries(subcommands: &'static [SubcommandSpec]) -> Vec<(String, String)> {
    subcommands
        .iter()
        .map(|subcommand| (subcommand.name.to_owned(), subcommand.summary.to_owned()))
        .collect()
}

fn find_subcommand(spec: &'static CommandSpec, name: &str) -> Option<&'static SubcommandSpec> {
    spec.subcommands
        .iter()
        .find(|subcommand| subcommand.name.eq_ignore_ascii_case(name))
}

/// 前缀匹配；无命中时退回编辑距离 ≤ 2 的拼写建议（`suggestion`）。
fn match_items(
    kind: CompletionKind,
    entries: &[(String, String)],
    token: &str,
) -> Vec<CompletionItem> {
    let lower = token.to_lowercase();
    let prefix_matches: Vec<CompletionItem> = entries
        .iter()
        .filter(|(text, _)| text.to_lowercase().starts_with(&lower))
        .take(MAX_ITEMS)
        .map(|(text, summary)| CompletionItem {
            text: text.clone(),
            kind,
            summary: summary.clone(),
        })
        .collect();

    if !prefix_matches.is_empty() || token.is_empty() {
        return prefix_matches;
    }

    let mut suggestions: Vec<(usize, &str, &str)> = entries
        .iter()
        .filter_map(|(text, summary)| {
            let distance = edit_distance(token, text);
            (distance > 0 && distance <= 2).then_some((distance, text.as_str(), summary.as_str()))
        })
        .collect();
    // 同距离按文本字典序，保证结果稳定。
    suggestions.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));

    suggestions
        .into_iter()
        .take(MAX_ITEMS)
        .map(|(_, text, summary)| CompletionItem {
            text: text.to_owned(),
            kind: CompletionKind::Suggestion,
            summary: summary.to_owned(),
        })
        .collect()
}

/// 动态序号候选：`1..=queueLength`，最多 [`MAX_ITEMS`] 条。
///
/// `None`（快照失败 / 队列长度未知）与 `Some(0)` 都返回空列表，不报错、不阻塞。
fn dynamic_items(queue_length: Option<usize>) -> Vec<CompletionItem> {
    let Some(length) = queue_length else {
        return Vec::new();
    };

    (1..=length)
        .take(MAX_ITEMS)
        .map(|index| CompletionItem {
            text: index.to_string(),
            kind: CompletionKind::Dynamic,
            summary: format!("队列第 {index} 首"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::json;

    use super::super::registry::{ArgSpec, FlagSpec};
    use super::*;

    /// 不带动态数据的补全（大多数用例）。
    fn complete_static(input: &str, cursor: usize) -> Completion {
        complete_with(registry(), input, cursor, || None)
    }

    fn texts(completion: &Completion) -> Vec<&str> {
        completion
            .items
            .iter()
            .map(|item| item.text.as_str())
            .collect()
    }

    #[test]
    fn empty_input_lists_every_command_once() {
        let completion = complete_static("", 0);

        let names = texts(&completion);
        assert_eq!(names.len(), registry().specs().len());
        for spec in registry().specs() {
            assert!(names.contains(&spec.name), "missing {}", spec.name);
        }
        // 别名不重复列出（`q` / `?` 不是独立候选）。
        assert!(!names.contains(&"q"));
        assert!(!names.contains(&"?"));
        assert!(
            completion
                .items
                .iter()
                .all(|item| item.kind == CompletionKind::Command)
        );
        assert_eq!((completion.replace_start, completion.replace_end), (0, 0));
    }

    #[test]
    fn prefix_filters_command_names() {
        let completion = complete_static("pl", 2);

        assert_eq!(texts(&completion), ["play"]);
        assert_eq!(completion.replace_start, 0);
        assert_eq!(completion.replace_end, 2);
        assert_eq!(completion.items[0].kind, CompletionKind::Command);
        assert!(!completion.items[0].summary.is_empty());
    }

    #[test]
    fn prefix_match_is_case_insensitive_and_returns_canonical_name() {
        let completion = complete_static("PL", 2);

        assert_eq!(texts(&completion), ["play"]);
    }

    #[test]
    fn no_prefix_match_falls_back_to_suggestions() {
        let completion = complete_static("plya", 4);

        assert_eq!(texts(&completion), ["play"]);
        assert_eq!(completion.items[0].kind, CompletionKind::Suggestion);
        assert_eq!((completion.replace_start, completion.replace_end), (0, 4));
    }

    #[test]
    fn suggestions_are_sorted_by_distance_then_name() {
        // 离 `queue` 1、离 `seek` 2、离 `next` 2：距离优先，其次字典序。
        let completion = complete_static("qeue", 4);
        assert_eq!(texts(&completion), ["queue"]);

        let completion = complete_static("seex", 4);
        assert_eq!(texts(&completion), ["seek"]);
    }

    #[test]
    fn unknown_command_without_near_miss_returns_nothing() {
        let completion = complete_static("xyz", 3);

        assert!(completion.items.is_empty());
        assert_eq!((completion.replace_start, completion.replace_end), (0, 3));
    }

    #[test]
    fn subcommands_follow_the_command_and_space() {
        let completion = complete_static("queue ", 6);

        assert_eq!(
            texts(&completion),
            ["add", "remove", "clear", "jump", "list"]
        );
        assert!(
            completion
                .items
                .iter()
                .all(|item| item.kind == CompletionKind::Subcommand)
        );
        assert_eq!((completion.replace_start, completion.replace_end), (6, 6));
    }

    #[test]
    fn subcommand_prefix_filters() {
        let completion = complete_static("queue r", 7);

        assert_eq!(texts(&completion), ["remove"]);
        assert_eq!(completion.replace_start, 6);
        assert_eq!(completion.replace_end, 7);
    }

    #[test]
    fn enum_values_follow_the_subcommand() {
        let completion = complete_static("mode repeat ", 12);

        assert_eq!(texts(&completion), ["off", "one", "all"]);
        assert!(
            completion
                .items
                .iter()
                .all(|item| item.kind == CompletionKind::EnumValue)
        );
        assert_eq!((completion.replace_start, completion.replace_end), (12, 12));
    }

    #[test]
    fn enum_value_prefix_filters() {
        let completion = complete_static("mode repeat o", 13);

        assert_eq!(texts(&completion), ["off", "one"]);
        assert_eq!((completion.replace_start, completion.replace_end), (12, 13));
    }

    #[test]
    fn dynamic_candidates_only_for_play_with_empty_token() {
        let mut polls = 0;
        let completion = complete_with(registry(), "play ", 5, || {
            polls += 1;
            Some(3)
        });

        assert_eq!(polls, 1);
        assert_eq!(texts(&completion), ["1", "2", "3"]);
        assert!(
            completion
                .items
                .iter()
                .all(|item| item.kind == CompletionKind::Dynamic)
        );
        assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
    }

    #[test]
    fn dynamic_candidates_cap_at_the_item_limit() {
        let completion = complete_with(registry(), "play ", 5, || Some(500));

        assert_eq!(completion.items.len(), MAX_ITEMS);
        assert_eq!(completion.items[0].text, "1");
        assert_eq!(completion.items[MAX_ITEMS - 1].text, MAX_ITEMS.to_string());
    }

    #[test]
    fn dynamic_candidates_degrade_when_queue_length_is_unknown() {
        let completion = complete_with(registry(), "play ", 5, || None);

        assert!(completion.items.is_empty());
        assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
    }

    #[test]
    fn dynamic_closure_is_not_polled_for_non_dynamic_positions() {
        let polls = Cell::new(0);
        let _ = complete_static("queue ", 6);
        let _ = complete_static("mode repeat ", 12);
        let _ = complete_static("vol 5", 5);
        let counted = complete_with(registry(), "pause", 5, || {
            polls.set(polls.get() + 1);
            Some(3)
        });

        assert!(
            counted
                .items
                .iter()
                .all(|item| item.kind != CompletionKind::Dynamic)
        );
        assert_eq!(polls.get(), 0);
    }

    #[test]
    fn replace_range_counts_characters_not_bytes() {
        // `中` 占 3 字节 / 1 字符：字节偏移会得到 0..6，字符偏移必须 0..4。
        let completion = complete_static("中lay", 4);

        assert_eq!(texts(&completion), ["play"]);
        assert_eq!(completion.items[0].kind, CompletionKind::Suggestion);
        assert_eq!(completion.replace_start, 0);
        assert_eq!(completion.replace_end, 4);
        assert_eq!("中lay".chars().count(), 4);
        assert_eq!("中lay".len(), 6);
    }

    #[test]
    fn cursor_in_the_middle_of_a_token_replaces_the_whole_token() {
        let completion = complete_static("play abc def", 6);

        assert_eq!((completion.replace_start, completion.replace_end), (5, 8));
        assert!(completion.items.is_empty());
    }

    #[test]
    fn cursor_past_the_end_is_clamped() {
        let completion = complete_static("pl", 99);

        assert_eq!(texts(&completion), ["play"]);
        assert_eq!((completion.replace_start, completion.replace_end), (0, 2));
    }

    #[test]
    fn whitespace_only_input_lists_commands_at_the_cursor() {
        let completion = complete_static("   ", 3);

        assert_eq!(completion.replace_start, 3);
        assert_eq!(completion.replace_end, 3);
        assert_eq!(texts(&completion).len(), registry().specs().len());
    }

    #[test]
    fn unsupported_argument_positions_have_no_candidates() {
        // 路径 / 播放位置 / 整数都不做补全（T-0020 §3 非目标）。
        assert!(complete_static("vol ", 4).items.is_empty());
        assert!(complete_static("seek ", 5).items.is_empty());
        assert!(complete_static("queue add ", 10).items.is_empty());
    }

    #[test]
    fn unknown_subcommand_has_no_candidates() {
        // `queue rmeove` 的子命令建议由解析器给，补全这里只认前缀/近拼写；
        // `rmeove` 离任何子命令都超过 2。
        assert!(complete_static("queue rmeove ", 13).items.is_empty());
    }

    #[test]
    fn completion_serializes_to_camel_case_json() {
        let completion = complete_static("pl", 2);
        let value = serde_json::to_value(&completion).expect("completion serializes");

        assert_eq!(
            value,
            json!({
                "replaceStart": 0,
                "replaceEnd": 2,
                "items": [{
                    "text": "play",
                    "kind": "command",
                    "summary": "播放队列中的曲目或文件；不带参数时继续当前曲目"
                }]
            })
        );
    }

    #[test]
    fn enum_kind_serializes_as_enum() {
        let completion = complete_static("mode repeat ", 12);
        let value = serde_json::to_value(&completion).expect("completion serializes");

        assert_eq!(value["items"][0]["kind"], json!("enum"));
    }

    /// 自定义命令表：覆盖 v1 表里没有的形状（带值标志等），
    /// 只用来锁住补全不碰标志、不 panic。
    const PROBE_SPECS: &[CommandSpec] = &[CommandSpec {
        name: "probe",
        aliases: &[],
        summary: "测试专用命令",
        usage: "probe [--limit=N]",
        args: &[ArgSpec {
            name: "value",
            kind: ArgKind::Text,
            required: false,
        }],
        flags: &[FlagSpec {
            name: "limit",
            takes_value: true,
            summary: "测试专用带值标志",
        }],
        subcommands: &[],
        default_subcommand: None,
    }];

    #[test]
    fn custom_registry_is_supported_without_touching_flags() {
        let registry = Registry::from_specs(PROBE_SPECS);
        let completion = complete_with(&registry, "probe ", 6, || None);

        assert!(completion.items.is_empty());
        assert_eq!((completion.replace_start, completion.replace_end), (6, 6));
    }
}
