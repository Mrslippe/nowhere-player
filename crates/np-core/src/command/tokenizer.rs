//! 命令分词器（T-0017 §6.1）。
//!
//! 规则只有三条，且是刻意的：
//!
//! 1. 空白（空格 / 制表符 / 换行）分隔 token，连续空白折叠。
//! 2. `"..."` 与 `'...'` 包裹的内容整体保留，可以含空白；引号内不处理转义。
//! 3. **反斜杠是字面量**，不转义任何字符——Windows 路径必须原样到达执行层。
//!
//! 引号可以从 token 中间开始或结束（`a"b c"d` → `ab cd`），
//! `starts_quoted` 只记录 token 是否以引号开头，供解析层区分
//! `"--json"`（位置参数）与 `--json`（标志）。

use super::ParseError;

/// 一个已经切开的输入片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    /// 去掉引号后的文本；反斜杠保持原样。
    pub text: String,
    /// token 是否以引号开头。以引号开头的 `--foo` 是位置参数，不是标志。
    pub starts_quoted: bool,
}

/// 按空白切分输入。
///
/// - 空输入 / 纯空白：`Ok(None)`。
/// - 引号未闭合：`Err(E_CMD_ARG)`，提示“引号未闭合”。
///
/// 引号字符本身不进入结果；反斜杠永远进入结果。
pub(crate) fn tokenize(input: &str) -> Result<Option<Vec<Token>>, ParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut starts_quoted = false;
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            if in_token {
                tokens.push(Token {
                    text: std::mem::take(&mut current),
                    starts_quoted,
                });
                in_token = false;
            }
            continue;
        }

        if !in_token {
            in_token = true;
            starts_quoted = ch == '"' || ch == '\'';
        }

        if ch == '"' || ch == '\'' {
            let quote = ch;
            loop {
                match chars.next() {
                    Some(next) if next == quote => break,
                    Some(next) => current.push(next),
                    None => return Err(ParseError::unclosed_quote(quote)),
                }
            }
        } else {
            // 反斜杠走这里：它是普通字符，绝不转义下一个字符。
            current.push(ch);
        }
    }

    if in_token {
        tokens.push(Token {
            text: current,
            starts_quoted,
        });
    }

    Ok(if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(tokens: &[Token]) -> Vec<&str> {
        tokens.iter().map(|token| token.text.as_str()).collect()
    }

    #[test]
    fn whitespace_runs_are_collapsed() {
        let tokens = tokenize("  play \t 3\n").unwrap().unwrap();

        assert_eq!(texts(&tokens), ["play", "3"]);
        assert!(tokens.iter().all(|token| !token.starts_quoted));
    }

    #[test]
    fn empty_and_whitespace_input_are_none() {
        assert_eq!(tokenize("").unwrap(), None);
        assert_eq!(tokenize(" \t\r\n ").unwrap(), None);
    }

    #[test]
    fn double_quotes_keep_spaces_in_one_token() {
        let tokens = tokenize(r#"play "C:\Music\My Music\a.flac""#)
            .unwrap()
            .unwrap();

        assert_eq!(texts(&tokens), ["play", r"C:\Music\My Music\a.flac"]);
        assert!(!tokens[0].starts_quoted);
        assert!(tokens[1].starts_quoted);
    }

    #[test]
    fn single_quotes_are_equivalent_to_double_quotes() {
        let tokens = tokenize("queue add '~/My Music/a.flac'").unwrap().unwrap();

        assert_eq!(texts(&tokens), ["queue", "add", "~/My Music/a.flac"]);
    }

    #[test]
    fn windows_backslashes_are_literal() {
        let tokens = tokenize(r"play C:\Music\a.flac").unwrap().unwrap();

        // 一个 token，反斜杠一个不少；如果有人把反斜杠当转义符，这里会先挂。
        assert_eq!(texts(&tokens), ["play", r"C:\Music\a.flac"]);
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn backslash_does_not_escape_the_closing_quote() {
        let tokens = tokenize(r#"play "C:\Music\""#).unwrap().unwrap();

        // 反斜杠是字面量，所以末尾的 `"` 正常闭合引号，token 以反斜杠结尾。
        assert_eq!(texts(&tokens), ["play", "C:\\Music\\"]);
        assert!(tokens[1].starts_quoted);
    }

    #[test]
    fn quote_in_the_middle_of_a_token_concatenates() {
        let tokens = tokenize(r#"a"b c"d"#).unwrap().unwrap();

        assert_eq!(texts(&tokens), ["ab cd"]);
        assert!(!tokens[0].starts_quoted);
    }

    #[test]
    fn quoted_flag_looking_token_is_marked_as_quoted() {
        let tokens = tokenize(r#"play "--json""#).unwrap().unwrap();

        assert_eq!(texts(&tokens), ["play", "--json"]);
        assert!(tokens[1].starts_quoted);
    }

    #[test]
    fn empty_quoted_token_is_still_a_token() {
        let tokens = tokenize(r#"play """#).unwrap().unwrap();

        assert_eq!(texts(&tokens), ["play", ""]);
        assert!(tokens[1].starts_quoted);
    }

    #[test]
    fn unclosed_quote_is_an_arg_error() {
        let error = tokenize(r#"play "C:\Music"#).unwrap_err();

        assert_eq!(error.code, super::super::CommandErrorCode::Arg);
        assert!(error.message.contains("引号未闭合"), "{}", error.message);
        assert!(error.usage.is_none());
    }
}
