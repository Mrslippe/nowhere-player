//! 单二进制启动器：双击时用 Windows Terminal 重新拉起自己。

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use np_app::SettingsHandle;

pub const LAUNCHED_ENV: &str = "NP_TUI_LAUNCHED";
pub const FALLBACK_ENV: &str = "NP_TUI_FALLBACK";

/// 是否运行在可交互终端里。测试通过 [`should_relaunch`] 注入。
pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn should_relaunch(is_tty: bool, already_launched: bool) -> bool {
    !is_tty && !already_launched
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentStatus {
    Created,
    Unchanged,
    Updated,
}

/// DES-009 的 fragment 内容；16 色必须齐全，UTF-8 无 BOM。
pub const FRAGMENT_JSON: &str = r##"{
  "profiles": [
    {
      "name": "Nowhere Player",
      "commandline": "nowhere-tui.exe",
      "colorScheme": "Nowhere Dark",
      "__FONT_LINE__"
      "padding": "0,0,0,0",
      "opacity": 96,
      "useAcrylic": true,
      "cursorShape": "filledBox",
      "antialiasingMode": "grayscale",
      "unfocusedAppearance": { "opacity": 70 },
      "suppressApplicationTitle": true,
      "scrollbarState": "hidden",
      "tabTitle": "Nowhere"
    }
  ],
  "schemes": [
    {
      "name": "Nowhere Dark",
      "background": "#0d0f12",
      "foreground": "#e9e4da",
      "black": "#0d0f12",
      "red": "#e06c5a",
      "green": "#9ec27f",
      "yellow": "#ffb454",
      "blue": "#7aa2f7",
      "purple": "#bb9af7",
      "cyan": "#7dd3c0",
      "white": "#e9e4da",
      "brightBlack": "#4d5560",
      "brightRed": "#f48771",
      "brightGreen": "#b9d98f",
      "brightYellow": "#ffc777",
      "brightBlue": "#9ab8ff",
      "brightPurple": "#d0b0ff",
      "brightCyan": "#9ee8d8",
      "brightWhite": "#f5f1ea"
    }
  ]
}
"##;

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn detect_font_face() -> Option<&'static str> {
    let mut dirs = Vec::new();
    if let Some(windir) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(windir).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
    }
    detect_font_in_dirs(&dirs)
}

fn detect_font_in_dirs(dirs: &[PathBuf]) -> Option<&'static str> {
    let mut has_sarasa = false;
    let mut has_maple = false;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.ends_with(".ttf") || name.ends_with(".otf") || name.ends_with(".ttc") {
                has_sarasa |= name.contains("sarasa");
                has_maple |= name.contains("maple");
            }
        }
    }
    if has_sarasa {
        Some("Sarasa Mono SC")
    } else if has_maple {
        Some("Maple Mono CN")
    } else {
        None
    }
}

pub fn font_warning() -> Option<&'static str> {
    detect_font_face()
        .is_none()
        .then_some("未检测到等宽中文字体，使用终端默认")
}

pub fn fragment_json_for(commandline: &str) -> String {
    fragment_json_for_with_font(commandline, detect_font_face())
}

fn fragment_json_for_with_font(commandline: &str, font: Option<&str>) -> String {
    let font_line = font
        .map(|face| format!("      \"font\": {{ \"face\": \"{face}\", \"size\": 12 }},\n"))
        .unwrap_or_default();
    FRAGMENT_JSON
        .replace("      \"__FONT_LINE__\"\n", &font_line)
        .replace("nowhere-tui.exe", &json_escape(commandline))
}

pub fn fragment_path(local_app_data: &Path) -> PathBuf {
    local_app_data
        .join("Microsoft")
        .join("Windows Terminal")
        .join("Fragments")
        .join("Nowhere")
        .join("nowhere.json")
}

/// 写入/更新 fragment；返回是否实际写入。
pub fn install_fragment_at(local_app_data: &Path) -> std::io::Result<FragmentStatus> {
    let commandline = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "nowhere-tui.exe".to_owned());
    install_fragment_at_with_command(local_app_data, &commandline)
}

fn install_fragment_at_with_command(
    local_app_data: &Path,
    commandline: &str,
) -> std::io::Result<FragmentStatus> {
    let path = fragment_path(local_app_data);
    let contents = fragment_json_for(commandline);
    if let Ok(existing) = std::fs::read(&path)
        && existing == contents.as_bytes()
    {
        return Ok(FragmentStatus::Unchanged);
    }
    let existed = path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents.as_bytes())?;
    Ok(if existed {
        FragmentStatus::Updated
    } else {
        FragmentStatus::Created
    })
}

pub fn install_fragment() -> std::io::Result<FragmentStatus> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA 未设置"))?;
    install_fragment_at(&base)
}

fn focus_mode_enabled() -> bool {
    let data_dir = std::env::var_os("NP_TUI_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let local = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("tmp/np-tui-data");
            if local.is_dir() {
                local
            } else {
                std::env::temp_dir().join("nowhere-tui")
            }
        });
    SettingsHandle::load(data_dir.join("config"))
        .get_bool("tui.focusMode")
        .unwrap_or(false)
}

fn log_line(message: &str) {
    let path = std::env::temp_dir().join("nowhere-tui-launch.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{message}");
    }
}

fn try_wt(args: &[String]) -> bool {
    match Command::new("wt.exe")
        .args(args)
        .env(LAUNCHED_ENV, "1")
        .output()
    {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            log_line(&format!(
                "[launcher] wt {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            false
        }
        Err(error) => {
            log_line(&format!("[launcher] wt spawn failed: {error}"));
            false
        }
    }
}

/// 双击路径：返回 true 表示父进程已成功拉起子进程，应退出。
pub fn relaunch_from_explorer() -> bool {
    let already_launched = std::env::var_os(LAUNCHED_ENV).is_some();
    if !should_relaunch(false, already_launched) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let _ = install_fragment();
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .into_owned();
    // 用新窗口 + 工作目录直接运行自身；--focus 是单用途播放器的默认外观。
    let mut args = vec![
        "-w".to_owned(),
        "new".to_owned(),
        "-p".to_owned(),
        "Nowhere Player".to_owned(),
    ];
    if focus_mode_enabled() {
        args.insert(0, "--focus".to_owned());
        if try_wt(&args) {
            return true;
        }
        // 旧版 WT 不认 --focus：记一行，再不带参数重试。
        args.remove(0);
    }
    if try_wt(&args) {
        return true;
    }
    // profile 不可用时退回默认 profile + 工作目录 + 绝对 exe。
    let fallback_args = vec![
        "-w".to_owned(),
        "new".to_owned(),
        "-d".to_owned(),
        cwd,
        exe.to_string_lossy().into_owned(),
    ];
    if try_wt(&fallback_args) {
        return true;
    }

    // wt 不存在/启动失败：回退到 conhost。子进程仍继承 marker，
    // 避免在无 TTY 环境里再次自举。
    Command::new("cmd.exe")
        .args(["/c", "start", "", &exe.to_string_lossy()])
        .env(LAUNCHED_ENV, "1")
        .env(FALLBACK_ENV, "1")
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_launcher_branches() {
        assert!(!should_relaunch(true, false));
        assert!(should_relaunch(false, false));
        assert!(!should_relaunch(false, true));
    }

    #[test]
    fn fragment_is_utf8_without_bom_and_has_all_sixteen_colors() {
        let dir = std::env::temp_dir().join(format!(
            "np-tui-fragment-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let status = install_fragment_at_with_command(&dir, "nowhere-tui.exe").expect("install");
        assert_eq!(status, FragmentStatus::Created);
        let path = fragment_path(&dir);
        let bytes = std::fs::read(&path).expect("read");
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.contains("\"padding\": \"0,0,0,0\""));
        assert!(!text.contains("__FONT_LINE__"));
        assert!(text.contains("\"scrollbarState\": \"hidden\""));
        for color in [
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "purple",
            "cyan",
            "white",
            "brightBlack",
            "brightRed",
            "brightGreen",
            "brightYellow",
            "brightBlue",
            "brightPurple",
            "brightCyan",
            "brightWhite",
        ] {
            assert!(text.contains(&format!("\"{color}\"")), "missing {color}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fragment_install_is_idempotent_and_updates_changed_content() {
        let dir = std::env::temp_dir().join(format!(
            "np-tui-fragment-idem-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(install_fragment_at(&dir).unwrap(), FragmentStatus::Created);
        assert_eq!(
            install_fragment_at(&dir).unwrap(),
            FragmentStatus::Unchanged
        );
        let path = fragment_path(&dir);
        std::fs::write(&path, b"different").unwrap();
        assert_eq!(
            install_fragment_at_with_command(&dir, "nowhere-tui.exe").unwrap(),
            FragmentStatus::Updated
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
