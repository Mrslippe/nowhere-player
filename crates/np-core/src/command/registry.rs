//! v1 命令表、按名/别名查找与拼写建议（T-0017 §2、§6.2）。
//!
//! 命令表是静态的：不做插件式注册，也不在这里执行任何命令。
//! 子命令（`queue add`）登记在父命令的 [`CommandSpec::subcommands`] 里，
//! 不是独立顶层命令。

/// 位置参数的形状。
///
/// 这里只区分“长得对不对”，不判断语义：`vol 999` 与 `seek 1:30` 都算形状合法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// 任意文本，可以含空格（路径、关键词）。
    Text,
    /// 有符号整数（`-10` / `+5` / `999`）；不校验范围。
    Integer,
    /// 播放位置：毫秒整数或 `mm:ss`，可带 `±`。
    SeekPosition,
    /// 闭集枚举（大小写不敏感），取值见参数。
    Enum(&'static [&'static str]),
}

impl ArgKind {
    /// 校验形状并返回要存进 [`crate::Parsed::positional`] 的值。
    ///
    /// 文本、整数与播放位置原样保留；枚举返回命令表里的**规范值**
    /// （大小写不敏感匹配，`ALL` → `all`），让执行层不必自己归一化。
    pub(crate) fn normalize(self, text: &str) -> Option<String> {
        match self {
            Self::Text => Some(text.to_owned()),
            Self::Integer if is_signed_integer(text) => Some(text.to_owned()),
            Self::Integer => None,
            Self::SeekPosition if is_seek_position(text) => Some(text.to_owned()),
            Self::SeekPosition => None,
            Self::Enum(values) => values
                .iter()
                .find(|value| value.eq_ignore_ascii_case(text))
                .map(|value| (*value).to_owned()),
        }
    }

    /// 给错误信息用的人话描述。
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Text => "文本".to_owned(),
            Self::Integer => "整数".to_owned(),
            Self::SeekPosition => "毫秒或 mm:ss".to_owned(),
            Self::Enum(values) => values.join("、"),
        }
    }
}

/// 一个位置参数的规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgSpec {
    /// 错误信息与帮助里显示的名字，如 `index|path`。
    pub name: &'static str,
    /// 形状。
    pub kind: ArgKind,
    /// 是否必填。可选参数必须排在必填参数之后（命令表自检会检查）。
    pub required: bool,
}

/// 一个 `--标志` 的规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagSpec {
    /// 不含 `--` 的标志名。
    pub name: &'static str,
    /// 是否必须带 `=值`。
    pub takes_value: bool,
    /// 一句话说明。
    pub summary: &'static str,
}

/// 子命令规格（`queue add` 的 `add`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubcommandSpec {
    /// 规范名（小写）。
    pub name: &'static str,
    /// 一句话说明。
    pub summary: &'static str,
    /// 完整用法，如 `queue add <index|path>`。
    pub usage: &'static str,
    /// 位置参数。
    pub args: &'static [ArgSpec],
    /// 允许的标志。
    pub flags: &'static [FlagSpec],
}

/// 一条顶层命令的规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// 规范名（小写）；查找大小写不敏感，但错误建议显示这个写法。
    pub name: &'static str,
    /// 显式登记的别名（如 `q`、`?`），不再支持前缀缩写。
    pub aliases: &'static [&'static str],
    /// 一句话说明。
    pub summary: &'static str,
    /// 完整用法。
    pub usage: &'static str,
    /// 自身的位置参数；有子命令时通常为空。
    pub args: &'static [ArgSpec],
    /// 允许的标志。
    pub flags: &'static [FlagSpec],
    /// 子命令表；为空表示这条命令没有子命令。
    pub subcommands: &'static [SubcommandSpec],
    /// 不带任何位置参数时采用的子命令（`queue` → `list`）。
    /// 为 `None` 时缺少子命令会报 `E_CMD_ARG`。
    pub default_subcommand: Option<&'static str>,
}

/// 命令表与查找入口。
#[derive(Debug, Clone, Copy)]
pub struct Registry {
    specs: &'static [CommandSpec],
}

impl Registry {
    /// 基于内置静态表构造（无 IO、无分配）。
    pub const fn new() -> Self {
        Self { specs: COMMANDS }
    }

    /// 全量命令表，供帮助与补全使用。
    pub fn specs(&self) -> &'static [CommandSpec] {
        self.specs
    }

    /// 按规范名或别名查找，大小写不敏感。
    pub fn find(&self, name: &str) -> Option<&'static CommandSpec> {
        self.specs.iter().find(|spec| {
            spec.name.eq_ignore_ascii_case(name)
                || spec
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(name))
        })
    }

    /// 拼写建议：编辑距离 ≤ 2，取最小者；同距离按规范名字典序。
    ///
    /// 返回规范写法（输入命中别名时也返回原名），供错误信息展示。
    pub fn suggest(&self, name: &str) -> Option<&'static CommandSpec> {
        let mut best: Option<(usize, &'static CommandSpec)> = None;

        for spec in self.specs {
            let distance = std::iter::once(spec.name)
                .chain(spec.aliases.iter().copied())
                .map(|candidate| edit_distance(name, candidate))
                .min()
                .unwrap_or(usize::MAX);

            if distance == 0 || distance > 2 {
                continue;
            }

            if best.is_none_or(|(best_distance, best_spec)| {
                (distance, spec.name) < (best_distance, best_spec.name)
            }) {
                best = Some((distance, spec));
            }
        }

        best.map(|(_, spec)| spec)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Registry {
    /// 测试专用：用自定义命令表构造，覆盖 v1 表里尚未出现的形状。
    pub(crate) const fn from_specs(specs: &'static [CommandSpec]) -> Self {
        Self { specs }
    }
}

/// 内置注册表的共享实例。
pub fn registry() -> &'static Registry {
    &REGISTRY
}

static REGISTRY: Registry = Registry::new();

/// 有符号整数：可选 `+` / `-`，其余必须全是 ASCII 数字。
///
/// 不解析成整数，所以超长数字也算形状合法，范围判断归执行层。
fn is_signed_integer(text: &str) -> bool {
    let digits = text
        .strip_prefix('+')
        .or_else(|| text.strip_prefix('-'))
        .unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// 播放位置：`[±]毫秒` 或 `[±]mm:ss`。
///
/// 只检查形状：`1:99` 也算合法，归一化与钳制归执行层。
fn is_seek_position(text: &str) -> bool {
    let body = text
        .strip_prefix('+')
        .or_else(|| text.strip_prefix('-'))
        .unwrap_or(text);
    if body.is_empty() {
        return false;
    }

    match body.split_once(':') {
        Some((minutes, seconds)) => {
            !minutes.is_empty()
                && !seconds.is_empty()
                && !seconds.contains(':')
                && minutes.bytes().all(|byte| byte.is_ascii_digit())
                && seconds.bytes().all(|byte| byte.is_ascii_digit())
        }
        None => body.bytes().all(|byte| byte.is_ascii_digit()),
    }
}

/// Levenshtein 编辑距离（大小写不敏感）。
pub(crate) fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.to_ascii_lowercase().chars().collect();
    let right: Vec<char> = right.to_ascii_lowercase().chars().collect();

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];

    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right.len()]
}

const QUEUE_SUBCOMMANDS: &[SubcommandSpec] = &[
    SubcommandSpec {
        name: "add",
        summary: "把曲目序号或文件路径加入队列",
        usage: "queue add <index|path>",
        args: &[ArgSpec {
            name: "index|path",
            kind: ArgKind::Text,
            required: true,
        }],
        flags: &[],
    },
    SubcommandSpec {
        name: "remove",
        summary: "从队列中移除指定序号",
        usage: "queue remove <index>",
        args: &[ArgSpec {
            name: "index",
            kind: ArgKind::Integer,
            required: true,
        }],
        flags: &[],
    },
    SubcommandSpec {
        name: "clear",
        summary: "清空队列",
        usage: "queue clear",
        args: &[],
        flags: &[],
    },
    SubcommandSpec {
        name: "jump",
        summary: "跳到队列中的指定序号",
        usage: "queue jump <index>",
        args: &[ArgSpec {
            name: "index",
            kind: ArgKind::Integer,
            required: true,
        }],
        flags: &[],
    },
    SubcommandSpec {
        name: "list",
        summary: "列出队列（等价于不带参数的 queue）",
        usage: "queue list",
        args: &[],
        flags: &[],
    },
];

const LIBRARY_SUBCOMMANDS: &[SubcommandSpec] = &[
    SubcommandSpec {
        name: "add",
        summary: "把一个目录加入媒体库扫描根",
        usage: "library add <path>",
        args: &[ArgSpec {
            name: "path",
            kind: ArgKind::Text,
            required: true,
        }],
        flags: &[],
    },
    SubcommandSpec {
        name: "scan",
        summary: "扫描媒体库（默认增量，--full 全量重解析）",
        usage: "library scan [--full]",
        args: &[],
        flags: &[FlagSpec {
            name: "full",
            takes_value: false,
            summary: "忽略 mtime 指纹，全部重新解析",
        }],
    },
    SubcommandSpec {
        name: "list",
        summary: "列出最近入库的曲目",
        usage: "library list [--limit=N]",
        args: &[],
        flags: &[FlagSpec {
            name: "limit",
            takes_value: true,
            summary: "返回条数（默认 20）",
        }],
    },
    SubcommandSpec {
        name: "stat",
        summary: "显示曲目 / 专辑 / 艺人 / 时长统计",
        usage: "library stat",
        args: &[],
        flags: &[],
    },
];

const MODE_SUBCOMMANDS: &[SubcommandSpec] = &[
    SubcommandSpec {
        name: "repeat",
        summary: "设置循环模式",
        usage: "mode repeat <off|one|all>",
        args: &[ArgSpec {
            name: "mode",
            kind: ArgKind::Enum(&["off", "one", "all"]),
            required: true,
        }],
        flags: &[],
    },
    SubcommandSpec {
        name: "shuffle",
        summary: "开关随机播放",
        usage: "mode shuffle <on|off>",
        args: &[ArgSpec {
            name: "switch",
            kind: ArgKind::Enum(&["on", "off"]),
            required: true,
        }],
        flags: &[],
    },
];

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "play",
        aliases: &[],
        summary: "播放队列中的曲目或文件；不带参数时继续当前曲目",
        usage: "play [index|path]",
        args: &[ArgSpec {
            name: "index|path",
            kind: ArgKind::Text,
            required: false,
        }],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "pause",
        aliases: &[],
        summary: "暂停当前曲目",
        usage: "pause",
        args: &[],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "resume",
        aliases: &[],
        summary: "继续播放",
        usage: "resume",
        args: &[],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "next",
        aliases: &[],
        summary: "切到下一首",
        usage: "next",
        args: &[],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "prev",
        aliases: &[],
        summary: "切到上一首",
        usage: "prev",
        args: &[],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "seek",
        aliases: &[],
        summary: "跳到指定播放位置",
        usage: "seek <ms|mm:ss|±s>",
        args: &[ArgSpec {
            name: "position",
            kind: ArgKind::SeekPosition,
            required: true,
        }],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "vol",
        aliases: &[],
        summary: "调整音量",
        usage: "vol <0-100|±N>",
        args: &[ArgSpec {
            name: "level",
            kind: ArgKind::Integer,
            required: true,
        }],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "queue",
        aliases: &["q"],
        summary: "查看或编辑播放队列",
        usage: "queue [add|remove|clear|jump|list]",
        args: &[],
        flags: &[],
        subcommands: QUEUE_SUBCOMMANDS,
        default_subcommand: Some("list"),
    },
    CommandSpec {
        name: "mode",
        aliases: &[],
        summary: "设置循环与随机模式",
        usage: "mode repeat <off|one|all> | mode shuffle <on|off>",
        args: &[],
        flags: &[],
        subcommands: MODE_SUBCOMMANDS,
        default_subcommand: None,
    },
    CommandSpec {
        name: "library",
        aliases: &["lib"],
        summary: "媒体库：添加扫描根、扫描、列表与统计",
        usage: "library [add|scan|list|stat]",
        args: &[],
        flags: &[],
        subcommands: LIBRARY_SUBCOMMANDS,
        default_subcommand: None,
    },
    CommandSpec {
        name: "search",
        aliases: &[],
        summary: "在媒体库中搜索曲目，结果可按序号播放",
        usage: "search <keyword>",
        args: &[ArgSpec {
            name: "keyword",
            kind: ArgKind::Text,
            required: true,
        }],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "help",
        aliases: &["?"],
        summary: "查看帮助或某条命令的用法",
        usage: "help [cmd] [--all]",
        args: &[ArgSpec {
            name: "cmd",
            kind: ArgKind::Text,
            required: false,
        }],
        flags: &[FlagSpec {
            name: "all",
            takes_value: false,
            summary: "列出隐藏命令",
        }],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "version",
        aliases: &[],
        summary: "显示版本",
        usage: "version",
        args: &[],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
    CommandSpec {
        name: "clear",
        aliases: &[],
        summary: "清空终端输出",
        usage: "clear",
        args: &[],
        flags: &[],
        subcommands: &[],
        default_subcommand: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_canonical_name() {
        let registry = Registry::new();

        for spec in registry.specs() {
            let found = registry
                .find(spec.name)
                .expect("canonical name must resolve");
            assert_eq!(found.name, spec.name);
        }
    }

    #[test]
    fn finds_registered_aliases() {
        let registry = Registry::new();

        assert_eq!(registry.find("q").map(|spec| spec.name), Some("queue"));
        assert_eq!(registry.find("?").map(|spec| spec.name), Some("help"));
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let registry = Registry::new();

        assert_eq!(registry.find("Play").map(|spec| spec.name), Some("play"));
        assert_eq!(registry.find("QUEUE").map(|spec| spec.name), Some("queue"));
        assert_eq!(registry.find("Q").map(|spec| spec.name), Some("queue"));
    }

    #[test]
    fn unknown_names_do_not_resolve() {
        let registry = Registry::new();

        assert!(registry.find("plya").is_none());
        assert!(registry.find("xyz").is_none());
        // 开发方案 §8.2 里尚未在本卡登记的命令也必须查不到。
        assert!(registry.find("toggle").is_none());
    }

    #[test]
    fn library_and_search_are_registered_with_their_alias() {
        let registry = Registry::new();

        assert_eq!(
            registry.find("library").map(|spec| spec.name),
            Some("library")
        );
        assert_eq!(registry.find("lib").map(|spec| spec.name), Some("library"));
        assert_eq!(
            registry.find("search").map(|spec| spec.name),
            Some("search")
        );
        assert_eq!(
            registry.find("library").map(|spec| spec.subcommands.len()),
            Some(4)
        );
    }

    #[test]
    fn suggests_the_nearest_command() {
        let registry = Registry::new();

        assert_eq!(registry.suggest("plya").map(|spec| spec.name), Some("play"));
        assert_eq!(
            registry.suggest("quueu").map(|spec| spec.name),
            Some("queue")
        );
        assert_eq!(registry.suggest("hepl").map(|spec| spec.name), Some("help"));
        assert_eq!(registry.suggest("SEEK").map(|spec| spec.name), None);
    }

    #[test]
    fn suggests_canonical_name_for_alias_typos() {
        let registry = Registry::new();

        // `qq` 离别名 `q` 最近，但建议必须显示规范名 `queue`。
        assert_eq!(registry.suggest("qq").map(|spec| spec.name), Some("queue"));
    }

    #[test]
    fn no_suggestion_beyond_edit_distance_two() {
        let registry = Registry::new();

        assert!(registry.suggest("xyz").is_none());
        assert!(registry.suggest("toggle").is_none());
    }

    #[test]
    fn command_table_is_self_consistent() {
        let mut names: Vec<&str> = Vec::new();

        for spec in COMMANDS {
            assert!(!spec.summary.is_empty(), "{} needs a summary", spec.name);
            assert!(
                spec.usage.starts_with(spec.name),
                "{} usage must start with its name",
                spec.name
            );

            assert!(
                names
                    .iter()
                    .all(|name| !name.eq_ignore_ascii_case(spec.name)),
                "duplicate command name {}",
                spec.name
            );
            names.push(spec.name);

            for alias in spec.aliases {
                assert!(
                    names.iter().all(|name| !name.eq_ignore_ascii_case(alias)),
                    "alias {alias} collides with a command name"
                );
                names.push(alias);
            }

            assert!(
                spec.subcommands.is_empty() || spec.args.is_empty(),
                "{} has both args and subcommands",
                spec.name
            );

            if let Some(default) = spec.default_subcommand {
                assert!(
                    spec.subcommands
                        .iter()
                        .any(|subcommand| subcommand.name == default),
                    "{} default subcommand {default} is missing",
                    spec.name
                );
            }

            let mut sub_names: Vec<&str> = Vec::new();
            for subcommand in spec.subcommands {
                assert!(
                    !subcommand.summary.is_empty(),
                    "{} {} needs a summary",
                    spec.name,
                    subcommand.name
                );
                assert!(
                    subcommand
                        .usage
                        .starts_with(&format!("{} {}", spec.name, subcommand.name)),
                    "{} {} usage is not prefixed correctly",
                    spec.name,
                    subcommand.name
                );
                assert!(
                    sub_names
                        .iter()
                        .all(|name| !name.eq_ignore_ascii_case(subcommand.name)),
                    "duplicate subcommand {} {}",
                    spec.name,
                    subcommand.name
                );
                sub_names.push(subcommand.name);
                assert_required_args_precede_optional(subcommand.args);
            }

            assert_required_args_precede_optional(spec.args);
        }
    }

    fn assert_required_args_precede_optional(args: &[ArgSpec]) {
        let mut seen_optional = false;
        for arg in args {
            if arg.required {
                assert!(
                    !seen_optional,
                    "required arg {} follows an optional one",
                    arg.name
                );
            } else {
                seen_optional = true;
            }
        }
    }

    #[test]
    fn arg_shapes_are_normalized_without_range_semantics() {
        assert_eq!(ArgKind::Integer.normalize("999"), Some("999".to_owned()));
        assert_eq!(ArgKind::Integer.normalize("+5"), Some("+5".to_owned()));
        assert_eq!(ArgKind::Integer.normalize("-5"), Some("-5".to_owned()));
        assert_eq!(ArgKind::Integer.normalize("1.5"), None);
        assert_eq!(ArgKind::Integer.normalize("abc"), None);
        assert_eq!(ArgKind::Integer.normalize(""), None);

        assert_eq!(
            ArgKind::SeekPosition.normalize("1:30"),
            Some("1:30".to_owned())
        );
        assert_eq!(
            ArgKind::SeekPosition.normalize("+10"),
            Some("+10".to_owned())
        );
        assert_eq!(
            ArgKind::SeekPosition.normalize("-10"),
            Some("-10".to_owned())
        );
        assert_eq!(
            ArgKind::SeekPosition.normalize("1:99"),
            Some("1:99".to_owned())
        );
        assert_eq!(ArgKind::SeekPosition.normalize("1:2:3"), None);
        assert_eq!(ArgKind::SeekPosition.normalize("abc"), None);

        assert_eq!(
            ArgKind::Enum(&["off", "one"]).normalize("ONE"),
            Some("one".to_owned())
        );
        assert_eq!(ArgKind::Enum(&["off", "one"]).normalize("all"), None);
    }
}
