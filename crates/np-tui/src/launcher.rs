//! 单二进制启动器：双击时用 Windows Terminal 重新拉起自己。

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

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
      "font": { "face": "Sarasa Mono SC", "size": 12 },
      "padding": "10,10,10,10",
      "opacity": 96,
      "useAcrylic": true,
      "cursorShape": "filledBox",
      "antialiasingMode": "grayscale",
      "unfocusedAppearance": { "opacity": 70 },
      "suppressApplicationTitle": true,
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
    let path = fragment_path(local_app_data);
    if let Ok(existing) = std::fs::read(&path)
        && existing == FRAGMENT_JSON.as_bytes()
    {
        return Ok(FragmentStatus::Unchanged);
    }
    let existed = path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, FRAGMENT_JSON.as_bytes())?;
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 用新窗口 + 工作目录直接运行自身，避免依赖 profile 的 commandline
    // 是否已在 PATH 中；fragment 仍会安装，用户也可从 WT 菜单选择带样式的 profile。
    let child = Command::new("wt.exe")
        .args([
            "-w".to_owned(),
            "new".to_owned(),
            "-d".to_owned(),
            cwd.to_string_lossy().into_owned(),
            exe.to_string_lossy().into_owned(),
        ])
        .env(LAUNCHED_ENV, "1")
        .spawn();
    if child.is_ok() {
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
        let status = install_fragment_at(&dir).expect("install");
        assert_eq!(status, FragmentStatus::Created);
        let path = fragment_path(&dir);
        let bytes = std::fs::read(&path).expect("read");
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        let text = String::from_utf8(bytes).expect("utf8");
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
        assert_eq!(install_fragment_at(&dir).unwrap(), FragmentStatus::Updated);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
