//! 开机自启：写 HKCU Run；配置键 `startup.enabled`。
//!
//! 自启只改当前用户的 Run 键，不需要管理员权限。写入是幂等的：
//! 值已经等于当前 exe 时不再重复 `reg add`；关闭时值不存在也算成功。

use std::path::Path;
use std::process::Command;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const DEFAULT_VALUE_NAME: &str = "NowherePlayer";

fn value_name() -> String {
    value_name_from(std::env::var("NP_TUI_STARTUP_KEY").ok())
}

/// 允许用 `NP_TUI_STARTUP_KEY` 覆盖值名；空字符串回退到默认值。
fn value_name_from(override_name: Option<String>) -> String {
    override_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_VALUE_NAME.to_owned())
}

/// Run 值：整个 exe 路径加双引号，路径含空格也不会被拆成多个参数。
fn command_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// 从 `reg query` 输出里解析指定值名的 REG_SZ 数据。
///
/// 输出形如：
/// ```text
/// HKEY_CURRENT_USER\...\Run
///     NowherePlayer    REG_SZ    "C:\...\np-tui.exe"
/// ```
fn parse_reg_sz(stdout: &str, value_name: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let (left, right) = line.split_once("REG_SZ")?;
        if left.trim() == value_name {
            Some(right.trim().to_owned())
        } else {
            None
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    None,
    Add,
    Delete,
}

/// 幂等决策：enable 且值已等于目标 → 不写；disable 且值不存在 → 不删。
fn planned_change(enabled: bool, current: Option<&str>, target: &str) -> Change {
    match (enabled, current) {
        (true, Some(value)) if value == target => Change::None,
        (true, _) => Change::Add,
        (false, Some(_)) => Change::Delete,
        (false, None) => Change::None,
    }
}

/// 读当前值；键 / 值不存在（`reg query` 非 0 退出）当作 `None`。
///
/// `reg query` 无法从退出码区分“不存在”和“无权限”，两者都返回非 0。
/// 这里按“值不存在”处理：开启会继续 add，关闭直接 Ok；真正的失败由
/// add / delete 返回 stderr。只有 `reg.exe` 根本启动不了才在这里报错。
fn current_value(name: &str) -> Result<Option<String>, String> {
    let output = Command::new("reg.exe")
        .args(["query", RUN_KEY, "/v", name])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_reg_sz(&String::from_utf8_lossy(&output.stdout), name))
}

fn run_reg(args: &[&str]) -> Result<(), String> {
    let output = Command::new("reg.exe")
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let target = command_value(&exe);
    let name = value_name();
    let current = current_value(&name)?;
    match planned_change(enabled, current.as_deref(), &target) {
        Change::None => Ok(()),
        Change::Add => run_reg(&[
            "add",
            RUN_KEY,
            "/v",
            name.as_str(),
            "/t",
            "REG_SZ",
            "/d",
            target.as_str(),
            "/f",
        ]),
        Change::Delete => run_reg(&["delete", RUN_KEY, "/v", name.as_str(), "/f"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERY_OUTPUT: &str = "\
HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run
    NowherePlayer    REG_SZ    \"C:\\Program Files\\Nowhere Player\\np-tui.exe\"
";

    #[test]
    fn env_override_keeps_custom_value_name() {
        assert_eq!(value_name_from(None), "NowherePlayer");
        assert_eq!(
            value_name_from(Some("NP-T0125-Test".to_owned())),
            "NP-T0125-Test"
        );
        assert_eq!(value_name_from(Some("   ".to_owned())), "NowherePlayer");
    }

    #[test]
    fn parse_reg_sz_finds_only_the_requested_value() {
        assert_eq!(
            parse_reg_sz(QUERY_OUTPUT, "NowherePlayer").as_deref(),
            Some("\"C:\\Program Files\\Nowhere Player\\np-tui.exe\"")
        );
        assert_eq!(parse_reg_sz(QUERY_OUTPUT, "Other"), None);
        assert_eq!(parse_reg_sz("", "NowherePlayer"), None);
        assert_eq!(
            parse_reg_sz("    NowherePlayer    REG_DWORD    0x1\n", "NowherePlayer"),
            None
        );
    }

    #[test]
    fn run_value_quotes_paths_with_spaces() {
        let exe = Path::new(r"C:\Program Files\Nowhere Player\np-tui.exe");
        assert_eq!(
            command_value(exe),
            "\"C:\\Program Files\\Nowhere Player\\np-tui.exe\""
        );
        assert_eq!(
            command_value(Path::new(r"C:\np-tui.exe")),
            "\"C:\\np-tui.exe\""
        );
    }

    #[test]
    fn planned_change_is_idempotent() {
        let target = "\"C:\\np-tui.exe\"";
        assert_eq!(planned_change(true, Some(target), target), Change::None);
        assert_eq!(planned_change(true, None, target), Change::Add);
        assert_eq!(
            planned_change(true, Some("\"D:\\old.exe\""), target),
            Change::Add
        );
        assert_eq!(planned_change(false, None, target), Change::None);
        assert_eq!(planned_change(false, Some(target), target), Change::Delete);
    }
}
