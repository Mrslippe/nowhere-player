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
    /// 位置参数（已去掉子命令名、引号与命令名本身）。
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
    for token in rest {
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
            raw_flags.push((name, value));
            continue;
        }
        words.push(token);
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
    for (arg, token) in args.iter().zip(positional) {
        if !arg.kind.accepts(&token.text) {
            return Err(ParseError::invalid_arg(
                arg.name,
                &arg.kind.describe(),
                &token.text,
                usage,
            ));
        }
    }

    Ok(Some(Parsed {
        spec,
        subcommand,
        positional: positional.iter().map(|token| token.text.clone()).collect(),
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
        assert_eq!(parsed.usage(), "queue add <index|path>");
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
        assert_eq!(error.usage.as_deref(), Some("queue add <index|path>"));
    }

    #[test]
    fn mode_parses_enums_without_touching_semantics() {
        let parsed = parse_ok("mode repeat ALL");

        assert_eq!(parsed.subcommand.unwrap().name, "repeat");
        assert_eq!(parsed.positional, ["ALL"]);

        let parsed = parse_ok("mode shuffle off");
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
}
