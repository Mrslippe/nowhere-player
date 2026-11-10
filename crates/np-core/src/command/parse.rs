//! 解析入口与参数校验（T-0017 §2、§6.3）。
//!
//! [`parse`] 把一行输入变成 [`Parsed`]：先按 [`crate::command::Registry`] 找命令，
//! 再识别子命令与标志，最后只校验参数**形状**。范围（`vol 999`）、当前状态
//! （空队列的 `next`）等语义问题一律留给执行层。

use std::collections::BTreeMap;

use super::registry::edit_distance;
use super::{
    CommandSpec, FlagSpec, ParseError, Registry, SubcommandSpec, Token, registry, tokenize,
};

/// 解析成功的命令，供执行层（T-0018）消费。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// 命中的顶层命令（规范写法，大小写已归一）。
    pub spec: &'static CommandSpec,
    /// 命中的子命令；`queue` 无参数时解析为默认的 `list`。
    pub subcommand: Option<&'static SubcommandSpec>,
    /// 位置参数（已去掉子命令名、引号与命令名本身）；枚举参数已归一化为规范值。
    pub positional: Vec<String>,
    /// 标志 → 取值；不带取值的标志（如 `--all`）映射到空字符串。
    pub flags: BTreeMap<String, String>,
}

impl Parsed {
    /// 当前命令（含子命令）的完整用法。
    pub fn usage(&self) -> &'static str {
        self.subcommand
            .map_or(self.spec.usage, |subcommand| subcommand.usage)
    }
}

/// 解析一行输入。
///
/// - 空输入 / 纯空白：`Ok(None)`。
/// - 语法错误：`Err(ParseError)`，错误码是 [`crate::CommandErrorCode`]。
pub fn parse(input: &str) -> Result<Option<Parsed>, ParseError> {
    parse_with(registry(), input)
}

fn parse_with(registry: &Registry, input: &str) -> Result<Option<Parsed>, ParseError> {
    let Some(tokens) = tokenize(input)? else {
        return Ok(None);
    };
    let (head, rest) = tokens
        .split_first()
        .expect("tokenize never returns an empty token list");

    let spec = registry.find(&head.text).ok_or_else(|| {
        ParseError::unknown_command(
            &head.text,
            registry.suggest(&head.text).map(|spec| spec.name),
        )
    })?;

    // 先分开“位置词”与 `--标志`：子命令从位置词里识别，
    // 引号开头的 `"--x"` 仍是位置参数。
    let mut words: Vec<&Token> = Vec::new();
    let mut raw_flags: Vec<(&str, Option<&str>)> = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        let token = &rest[index];
        let flag_body = if token.starts_quoted {
            None
        } else {
            token.text.strip_prefix("--")
        };
        if let Some(body) = flag_body {
            let (name, value) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, None),
            };
            // 带值标志也接受空格形式（`--limit 20`）；下一个 token 不是
            // 新标志时当作值消费，避免它落进位置参数。
            if value.is_none()
                && flag_takes_value(spec, name)
                && let Some(next) = rest.get(index + 1)
                && !next.text.starts_with("--")
            {
                raw_flags.push((name, Some(next.text.as_str())));
                index += 2;
                continue;
            }
            raw_flags.push((name, value));
            index += 1;
            continue;
        }
        words.push(token);
        index += 1;
    }

    let (subcommand, positional) = resolve_subcommand(spec, &words)?;
    let usage = subcommand.map_or(spec.usage, |subcommand| subcommand.usage);

    let mut flags = BTreeMap::new();
    for (name, value) in raw_flags {
        if name.is_empty() {
            return Err(ParseError::empty_flag_name(usage));
        }
        let Some(flag) = find_flag(spec, subcommand, name) else {
            return Err(ParseError::unknown_flag(name, usage));
        };
        let stored = match (flag.takes_value, value) {
            (true, Some(value)) if !value.is_empty() => value.to_owned(),
            (true, _) => return Err(ParseError::flag_missing_value(name, usage)),
            (false, None) => String::new(),
            (false, Some(_)) => return Err(ParseError::flag_unexpected_value(name, usage)),
        };
        flags.insert(name.to_owned(), stored);
    }

    let args = subcommand.map_or(spec.args, |subcommand| subcommand.args);
    let required = args.iter().filter(|arg| arg.required).count();
    if positional.len() < required {
        return Err(ParseError::missing_arg(args[positional.len()].name, usage));
    }
    if positional.len() > args.len() {
        return Err(ParseError::extra_arg(&positional[args.len()].text, usage));
    }
    let mut values = Vec::with_capacity(positional.len());
    for (arg, token) in args.iter().zip(positional) {
        match arg.kind.normalize(&token.text) {
            Some(value) => values.push(value),
            None => {
                return Err(ParseError::invalid_arg(
                    arg.name,
                    &arg.kind.describe(),
                    &token.text,
                    usage,
                ));
            }
        }
    }

    Ok(Some(Parsed {
        spec,
        subcommand,
        positional: values,
        flags,
    }))
}

fn resolve_subcommand<'words, 'token>(
    spec: &'static CommandSpec,
    words: &'words [&'token Token],
) -> Result<(Option<&'static SubcommandSpec>, &'words [&'token Token]), ParseError> {
    if spec.subcommands.is_empty() {
        return Ok((None, words));
    }

    match words.split_first() {
        Some((first, rest)) => match find_subcommand(spec, &first.text) {
            Some(subcommand) => Ok((Some(subcommand), rest)),
            None => Err(ParseError::unknown_subcommand(
                spec.name,
                &first.text,
                suggest_subcommand(spec, &first.text),
                spec.usage,
            )),
        },
        None => match spec
            .default_subcommand
            .and_then(|name| find_subcommand(spec, name))
        {
            Some(subcommand) => Ok((Some(subcommand), &[])),
            None => Err(ParseError::missing_subcommand(spec.name, spec.usage)),
        },
    }
}

fn find_subcommand<'a>(spec: &'a CommandSpec, name: &str) -> Option<&'a SubcommandSpec> {
    spec.subcommands
        .iter()
        .find(|subcommand| subcommand.name.eq_ignore_ascii_case(name))
}

fn flag_takes_value(spec: &CommandSpec, name: &str) -> bool {
    spec.flags
        .iter()
        .any(|flag| flag.name == name && flag.takes_value)
        || spec
            .subcommands
            .iter()
            .flat_map(|subcommand| subcommand.flags.iter())
            .any(|flag| flag.name == name && flag.takes_value)
}

fn find_flag<'a>(
    spec: &'a CommandSpec,
    subcommand: Option<&'a SubcommandSpec>,
    name: &str,
) -> Option<&'a FlagSpec> {
    let parent = spec.flags.iter().find(|flag| flag.name == name);
    parent.or_else(|| {
        subcommand.and_then(|subcommand| subcommand.flags.iter().find(|flag| flag.name == name))
    })
}

fn suggest_subcommand<'a>(spec: &'a CommandSpec, name: &str) -> Option<&'a str> {
    spec.subcommands
        .iter()
        .map(|subcommand| (edit_distance(name, subcommand.name), subcommand.name))
        .filter(|(distance, _)| *distance > 0 && *distance <= 2)
        .min()
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandErrorCode;

    /// v1 表里没有带值标志，测试专用一个，覆盖 `takes_value = true` 分支。
    const PROBE_SPECS: &[CommandSpec] = &[CommandSpec {
        name: "probe",
        aliases: &[],
        summary: "测试专用命令",
        usage: "probe [--limit=N]",
        args: &[],
        flags: &[FlagSpec {
            name: "limit",
            takes_value: true,
            summary: "测试专用带值标志",
        }],
        subcommands: &[],
        default_subcommand: None,
    }];

    fn parse_ok(input: &str) -> Parsed {
        parse(input)
            .unwrap_or_else(|error| panic!("{input:?} should parse, got {error}"))
            .expect("non-empty input must produce a command")
    }

    fn parse_err(input: &str) -> ParseError {
        parse(input).expect_err(&format!("{input:?} should fail"))
    }

    #[test]
    fn empty_input_is_none() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse(" \t\r\n ").unwrap(), None);
    }

    #[test]
    fn parses_command_without_arguments() {
        let parsed = parse_ok("pause");

        assert_eq!(parsed.spec.name, "pause");
        assert!(parsed.subcommand.is_none());
        assert!(parsed.positional.is_empty());
        assert!(parsed.flags.is_empty());
        assert_eq!(parsed.usage(), "pause");
    }

    #[test]
    fn parses_play_with_index() {
        let parsed = parse_ok("play 3");

        assert_eq!(parsed.spec.name, "play");
        assert_eq!(parsed.positional, ["3"]);
        assert!(!parsed.flags.contains_key("3"));
    }

    #[test]
    fn parses_play_with_windows_path_verbatim() {
        let parsed = parse_ok(r"play C:\Music\a.flac");

        // 反斜杠是字面量：如果有人把它当转义符，这个断言会先挂。
        assert_eq!(parsed.positional, [r"C:\Music\a.flac"]);
    }

    #[test]
    fn parses_quoted_path_with_spaces() {
        let parsed = parse_ok(r#"queue add "C:\Music\My Music\a.flac""#);

        assert_eq!(parsed.spec.name, "queue");
        assert_eq!(parsed.subcommand.unwrap().name, "add");
        assert_eq!(parsed.positional, [r"C:\Music\My Music\a.flac"]);
        assert_eq!(
            parsed.usage(),
            "queue add <path> | queue add --from-search <n>"
        );
    }

    #[test]
    fn quoted_flag_looking_argument_stays_positional() {
        let parsed = parse_ok(r#"play "--json""#);

        assert_eq!(parsed.positional, ["--json"]);
        assert!(parsed.flags.is_empty());
    }

    #[test]
    fn command_lookup_is_case_insensitive_and_returns_canonical_spec() {
        let parsed = parse_ok("Play 3");

        assert_eq!(parsed.spec.name, "play");
        assert_eq!(parsed.positional, ["3"]);
    }

    #[test]
    fn unknown_command_suggests_the_canonical_name() {
        let error = parse_err("plya");

        assert_eq!(error.code, CommandErrorCode::Unknown);
        assert!(error.message.contains("plya"), "{}", error.message);
        assert_eq!(error.suggestion.as_deref(), Some("play"));
        assert!(error.usage.is_none());
        assert!(
            error.to_string().contains("did you mean 'play'?"),
            "{error}"
        );
    }

    #[test]
    fn unclosed_quote_is_an_arg_error() {
        let error = parse_err(r#"play "C:\Music"#);

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("引号未闭合"), "{}", error.message);
    }

    #[test]
    fn missing_argument_reports_usage() {
        let error = parse_err("seek");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("缺少参数"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("seek <ms|mm:ss|±s>"));
    }

    #[test]
    fn extra_argument_is_rejected() {
        let error = parse_err("pause now");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("多余的参数"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("pause"));

        let error = parse_err("play 3 4");
        assert!(error.message.contains('4'), "{}", error.message);
    }

    #[test]
    fn non_integer_volume_is_rejected() {
        let error = parse_err("vol loud");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("整数"), "{}", error.message);
        assert!(error.message.contains("loud"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("vol <0-100|±N>"));
    }

    #[test]
    fn out_of_range_volume_still_parses() {
        // 解析层只管形状；范围由执行层钳制。
        assert_eq!(parse_ok("vol 999").positional, ["999"]);
        assert_eq!(parse_ok("vol -120").positional, ["-120"]);
    }

    #[test]
    fn seek_accepts_milliseconds_and_mm_ss() {
        for (input, position) in [
            ("seek 90000", "90000"),
            ("seek 1:30", "1:30"),
            ("seek +10", "+10"),
            ("seek -5", "-5"),
        ] {
            assert_eq!(parse_ok(input).positional, [position], "{input}");
        }
    }

    #[test]
    fn seek_rejects_bad_shape() {
        let error = parse_err("seek 1:2:3");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("毫秒或 mm:ss"), "{}", error.message);

        assert_eq!(parse_err("seek abc").code, CommandErrorCode::Arg);
    }

    #[test]
    fn queue_without_arguments_resolves_to_list() {
        let parsed = parse_ok("queue");

        assert_eq!(parsed.spec.name, "queue");
        assert_eq!(parsed.subcommand.unwrap().name, "list");
        assert!(parsed.positional.is_empty());
        assert_eq!(parsed.usage(), "queue list");
    }

    #[test]
    fn queue_alias_and_subcommand_are_resolved() {
        let parsed = parse_ok("q add 3");

        assert_eq!(parsed.spec.name, "queue");
        assert_eq!(parsed.subcommand.unwrap().name, "add");
        assert_eq!(parsed.positional, ["3"]);
    }

    #[test]
    fn queue_subcommand_lookup_is_case_insensitive() {
        let parsed = parse_ok("queue ADD 3");

        assert_eq!(parsed.subcommand.unwrap().name, "add");
    }

    #[test]
    fn queue_unknown_subcommand_suggests_close_match() {
        let error = parse_err("queue ad");

        assert_eq!(error.code, CommandErrorCode::Unknown);
        assert_eq!(error.suggestion.as_deref(), Some("add"));
        assert_eq!(
            error.usage.as_deref(),
            Some("queue [add|remove|clear|jump|list]")
        );
        assert!(parse_err("queue bogus").suggestion.is_none());
    }

    #[test]
    fn queue_add_rejects_extra_arguments() {
        let error = parse_err("queue add a b");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains('b'), "{}", error.message);
        assert_eq!(
            error.usage.as_deref(),
            Some("queue add <path> | queue add --from-search <n>")
        );
    }

    #[test]
    fn queue_add_from_search_flag_carries_number() {
        let parsed = parse_ok("queue add --from-search 2");

        assert!(parsed.positional.is_empty());
        assert_eq!(
            parsed.flags.get("from-search").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn queue_add_from_search_equals_form_is_accepted() {
        let parsed = parse_ok("queue add --from-search=3");

        assert!(parsed.positional.is_empty());
        assert_eq!(
            parsed.flags.get("from-search").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn queue_add_from_search_without_value_is_parse_error() {
        let error = parse_err("queue add --from-search");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("缺少取值"), "{}", error.message);
    }

    #[test]
    fn queue_add_allows_one_path_without_flag() {
        let parsed = parse_ok("queue add /music/a.flac");

        assert_eq!(parsed.positional, ["/music/a.flac"]);
        assert!(parsed.flags.is_empty());
    }

    #[test]
    fn queue_add_no_argument_parses_for_executor_to_reject() {
        let parsed = parse_ok("queue add");

        assert!(parsed.positional.is_empty());
        assert!(parsed.flags.is_empty());
    }

    #[test]
    fn queue_add_path_plus_flag_parses_for_executor_to_reject() {
        let parsed = parse_ok("queue add a.flac --from-search 1");

        assert_eq!(parsed.positional, ["a.flac"]);
        assert_eq!(
            parsed.flags.get("from-search").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn queue_add_rejects_two_paths() {
        let error = parse_err("queue add a.flac b.flac");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("b.flac"), "{}", error.message);
    }

    #[test]
    fn queue_add_keeps_quoted_path_with_spaces() {
        let parsed = parse_ok(r#"queue add "C:\Music\My Music\a.flac""#);

        assert_eq!(parsed.positional, [r"C:\Music\My Music\a.flac"]);
    }

    #[test]
    fn mode_normalizes_enum_values() {
        let parsed = parse_ok("mode repeat ALL");

        assert_eq!(parsed.subcommand.unwrap().name, "repeat");
        assert_eq!(parsed.positional, ["all"]);

        let parsed = parse_ok("mode shuffle OFF");
        assert_eq!(parsed.subcommand.unwrap().name, "shuffle");
        assert_eq!(parsed.positional, ["off"]);
    }

    #[test]
    fn mode_rejects_unknown_enum_value() {
        let error = parse_err("mode repeat loop");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("off、one、all"), "{}", error.message);
    }

    #[test]
    fn mode_requires_a_subcommand() {
        let error = parse_err("mode");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("缺少子命令"), "{}", error.message);
        assert_eq!(
            error.usage.as_deref(),
            Some("mode repeat <off|one|all> | mode shuffle <on|off>")
        );
    }

    #[test]
    fn help_collects_bare_flags_as_empty_strings() {
        let parsed = parse_ok("help --all");

        assert!(parsed.positional.is_empty());
        assert_eq!(parsed.flags.get("all").map(String::as_str), Some(""));
    }

    #[test]
    fn help_accepts_an_optional_topic() {
        assert_eq!(parse_ok("help play").positional, ["play"]);
    }

    #[test]
    fn unknown_flag_is_rejected_with_usage() {
        let error = parse_err("play --json");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("未知标志"), "{}", error.message);
        assert!(error.message.contains("--json"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("play [index|path]"));
    }

    #[test]
    fn boolean_flag_rejects_a_value() {
        let error = parse_err("help --all=yes");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("不接受取值"), "{}", error.message);
    }

    #[test]
    fn value_flags_accept_equals_and_space_forms() {
        let registry = Registry::from_specs(PROBE_SPECS);

        let equals = parse_with(&registry, "probe --limit=20")
            .unwrap()
            .expect("probe should parse");
        assert_eq!(equals.flags.get("limit").map(String::as_str), Some("20"));

        let space = parse_with(&registry, "probe --limit 20")
            .unwrap()
            .expect("space form should parse");
        assert_eq!(space.flags.get("limit").map(String::as_str), Some("20"));
        assert!(space.positional.is_empty(), "值不能落进位置参数");

        let error = parse_with(&registry, "probe --limit").unwrap_err();
        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("缺少取值"), "{}", error.message);
    }

    #[test]
    fn display_includes_usage_on_a_second_line() {
        let error = parse_err("vol");

        let rendered = error.to_string();
        assert!(rendered.contains("缺少参数"), "{rendered}");
        assert!(rendered.contains("用法：vol <0-100|±N>"), "{rendered}");
    }

    #[test]
    fn empty_quoted_command_name_is_rejected() {
        let error = parse_err(r#""""#);

        assert_eq!(error.code, CommandErrorCode::Unknown);
        assert!(error.message.contains("命令不能为空"), "{}", error.message);
    }

    #[test]
    fn library_requires_a_subcommand() {
        let error = parse_err("library");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("缺少子命令"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("library [add|scan|list|stat]"));
    }

    #[test]
    fn library_add_keeps_windows_path_verbatim() {
        let parsed = parse_ok(r"library add C:\Music\Library");

        assert_eq!(parsed.spec.name, "library");
        assert_eq!(parsed.subcommand.unwrap().name, "add");
        assert_eq!(parsed.positional, [r"C:\Music\Library"]);
        assert_eq!(parsed.usage(), "library add <path>");
    }

    #[test]
    fn library_add_requires_a_path() {
        let error = parse_err("library add");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("path"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("library add <path>"));
    }

    #[test]
    fn library_scan_accepts_a_bare_full_flag() {
        let parsed = parse_ok("library scan --full");

        assert_eq!(parsed.subcommand.unwrap().name, "scan");
        assert!(parsed.positional.is_empty());
        assert_eq!(parsed.flags.get("full").map(String::as_str), Some(""));
        assert_eq!(parsed.usage(), "library scan [--full]");
    }

    #[test]
    fn library_scan_rejects_a_value_on_full() {
        let error = parse_err("library scan --full=yes");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("不接受取值"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("library scan [--full]"));
    }

    #[test]
    fn library_list_reads_the_limit_flag() {
        let parsed = parse_ok("library list --limit=20");

        assert_eq!(parsed.subcommand.unwrap().name, "list");
        assert_eq!(parsed.flags.get("limit").map(String::as_str), Some("20"));
        assert_eq!(parsed.usage(), "library list [--limit=N]");
    }

    #[test]
    fn library_list_rejects_unknown_flags() {
        let error = parse_err("library list --json");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("未知标志"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("library list [--limit=N]"));
    }

    #[test]
    fn library_alias_keeps_the_subcommand() {
        let parsed = parse_ok("lib stat");

        assert_eq!(parsed.spec.name, "library");
        assert_eq!(parsed.subcommand.unwrap().name, "stat");
        assert_eq!(parsed.usage(), "library stat");
    }

    #[test]
    fn search_keeps_a_quoted_keyword() {
        let parsed = parse_ok(r#"search "kid a""#);

        assert_eq!(parsed.spec.name, "search");
        assert_eq!(parsed.positional, ["kid a"]);
        assert_eq!(parsed.usage(), "search <keyword> [--local]");
    }

    #[test]
    fn search_requires_a_keyword() {
        let error = parse_err("search");

        assert_eq!(error.code, CommandErrorCode::Arg);
        assert!(error.message.contains("keyword"), "{}", error.message);
        assert_eq!(error.usage.as_deref(), Some("search <keyword> [--local]"));
    }

    #[test]
    fn search_local_flag_is_accepted_before_or_after_keyword() {
        for input in ["search 晴天 --local", "search --local 晴天"] {
            let parsed = parse_ok(input);
            assert_eq!(parsed.spec.name, "search");
            assert_eq!(parsed.positional, ["晴天"]);
            assert!(parsed.flags.contains_key("local"));
        }
    }

    #[test]
    fn search_local_flag_rejects_a_value() {
        let error = parse_err("search 晴天 --local=1");

        assert_eq!(error.code, crate::CommandErrorCode::Arg);
        assert!(error.message.contains("不接受取值"));
    }

    #[test]
    fn parses_theme_family() {
        let list = parse_ok("theme");
        assert_eq!(list.spec.name, "theme");
        assert_eq!(list.subcommand.map(|sub| sub.name), Some("list"));

        let set = parse_ok("theme set ice");
        assert_eq!(set.subcommand.map(|sub| sub.name), Some("set"));
        assert_eq!(set.positional, ["ice"]);

        let export = parse_ok("theme export amber");
        assert_eq!(export.subcommand.map(|sub| sub.name), Some("export"));
    }

    #[test]
    fn parses_config_family() {
        let set = parse_ok("config set playback.crossfadeMs 300");
        assert_eq!(set.spec.name, "config");
        assert_eq!(set.subcommand.map(|sub| sub.name), Some("set"));
        assert_eq!(set.positional, ["playback.crossfadeMs", "300"]);

        let reset = parse_ok("config reset");
        assert_eq!(reset.subcommand.map(|sub| sub.name), Some("reset"));
    }

    #[test]
    fn parses_source_and_cache_families() {
        let disable = parse_ok("source disable bilibili");
        assert_eq!(disable.spec.name, "source");
        assert_eq!(disable.subcommand.map(|sub| sub.name), Some("disable"));
        assert_eq!(disable.positional, ["bilibili"]);

        let stat = parse_ok("cache");
        assert_eq!(stat.subcommand.map(|sub| sub.name), Some("stat"));
        let clear = parse_ok("cache clear");
        assert_eq!(clear.subcommand.map(|sub| sub.name), Some("clear"));
        assert!(clear.positional.is_empty());
        let all = parse_ok("cache clear all");
        assert_eq!(all.positional, ["all"]);
    }

    #[test]
    fn cache_clear_rejects_unknown_target() {
        let error = parse_err("cache clear nope");
        assert_eq!(error.code, crate::CommandErrorCode::Arg);
        assert_eq!(
            error.usage.as_deref(),
            Some("cache clear [streams|covers|all]")
        );
    }
}
