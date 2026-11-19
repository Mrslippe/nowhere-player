//! 应用内 mpv 下载/安装通道。
//!
//! 只使用 Windows 自带的 `curl`、`tar`、`certutil`，不新增 Rust 依赖。
//! 安装到 `%LOCALAPPDATA%\NowherePlayer\mpv\current`，`np-video` 会自动发现。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};

const VERSION: &str = "git-release-clang";
const ARCHIVE: &str = "https://github.com/mpv-player/mpv/releases/download/git-release/mpv-v0.42.0-dev-gb7339522c-44538826392-x86_64-pc-windows-msvc.zip";
const SHA256: &str = "673e3048c610aabb449876d9398edcb2223bc11a5c09b73ca043329e3c88ec8c";

type InstallRx = Receiver<Result<PathBuf, String>>;

fn state() -> &'static Mutex<Option<InstallRx>> {
    static STATE: OnceLock<Mutex<Option<InstallRx>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// 下载安装用的根目录；`current` 是最终可执行文件目录。
pub(crate) fn install_root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位 LOCALAPPDATA".to_owned())?;
    Ok(base.join("NowherePlayer").join("mpv").join("current"))
}

/// 已安装的 mpv 路径；文件存在才返回。
pub(crate) fn managed_mpv() -> Option<PathBuf> {
    let path = install_root().ok()?.join("mpv.exe");
    path.is_file().then_some(path)
}

/// 启动后台下载；已有任务或已安装时直接返回。
pub(crate) fn start() -> Result<(), String> {
    if managed_mpv().is_some() {
        return Err("mpv 已安装，无需重复下载".to_owned());
    }
    let mut slot = state()
        .lock()
        .map_err(|_| "mpv 安装状态锁已损坏".to_owned())?;
    if slot.is_some() {
        return Err("mpv 正在下载，请稍候".to_owned());
    }
    let (tx, rx) = mpsc::channel();
    *slot = Some(rx);
    std::thread::Builder::new()
        .name("nowhere-mpv-install".to_owned())
        .spawn(move || {
            let _ = tx.send(install());
        })
        .map_err(|error| format!("mpv 下载线程启动失败：{error}"))?;
    Ok(())
}

/// 轮询安装结果；只在完成/失败时返回一次状态。
pub(crate) fn poll() -> Option<Result<PathBuf, String>> {
    let mut slot = state().lock().ok()?;
    let receiver = slot.as_ref()?;
    match receiver.try_recv() {
        Ok(result) => {
            *slot = None;
            Some(result)
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            *slot = None;
            Some(Err("mpv 下载线程意外结束".to_owned()))
        }
    }
}

fn install() -> Result<PathBuf, String> {
    let root = install_root()?;
    let parent = root.parent().ok_or_else(|| "mpv 安装目录无效".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建 mpv 目录失败：{error}"))?;

    let archive = parent.join(format!("mpv-{VERSION}.zip"));
    let _ = std::fs::remove_file(&archive);
    let download = Command::new("curl")
        .args([
            "-L",
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            "3",
            "--connect-timeout",
            "15",
            "--max-time",
            "600",
            "-o",
        ])
        .arg(&archive)
        .arg(ARCHIVE)
        .status()
        .map_err(|error| format!("启动 curl 失败：{error}"))?;
    if !download.success() {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("mpv 下载失败（curl 退出码 {:?}）", download.code()));
    }

    let actual = sha256(&archive)?;
    if !actual.eq_ignore_ascii_case(SHA256) {
        let _ = std::fs::remove_file(&archive);
        return Err("mpv 安装包校验失败".to_owned());
    }

    let staging = parent.join("current.tmp");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| format!("创建 mpv 临时目录失败：{error}"))?;
    let unpack = Command::new("tar")
        .args(["-xf"])
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|error| format!("启动 tar 失败：{error}"))?;
    if !unpack.success() {
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_file(&archive);
        return Err("mpv 安装包解压失败".to_owned());
    }
    let _ = std::fs::remove_file(&archive);

    let executable = staging.join("mpv.exe");
    if !executable.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("mpv 安装包中没有 mpv.exe".to_owned());
    }
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|error| format!("替换旧 mpv 失败：{error}"))?;
    }
    std::fs::rename(&staging, &root).map_err(|error| format!("启用新 mpv 失败：{error}"))?;
    let installed = root.join("mpv.exe");
    installed
        .is_file()
        .then_some(installed)
        .ok_or_else(|| "mpv 安装后文件缺失".to_owned())
}

fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("certutil")
        .args(["-hashfile"])
        .arg(path)
        .arg("SHA256")
        .output()
        .map_err(|error| format!("启动 certutil 失败：{error}"))?;
    if !output.status.success() {
        return Err("计算 mpv 安装包校验值失败".to_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|part| part.len() == 64 && part.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| "无法读取 mpv 安装包校验值".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_constants_are_pinned_and_sha256_is_hex() {
        assert!(ARCHIVE.ends_with(".zip"));
        assert_eq!(SHA256.len(), 64);
        assert!(SHA256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn managed_path_is_under_local_app_data() {
        let root = install_root().expect("root");
        assert!(root.ends_with("NowherePlayer/mpv/current"));
    }

    /// 手动验证安装包解压；默认忽略，避免测试触网。
    #[test]
    #[ignore]
    fn extracts_the_pinned_archive() {
        let archive = std::env::var_os("NP_MPV_TEST_ARCHIVE").expect("archive env");
        let dir = std::env::temp_dir().join(format!("np-mpv-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let status = Command::new("tar")
            .args(["-xf"])
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .status()
            .expect("tar");
        assert!(status.success());
        assert!(
            dir.join("mpv.exe").is_file(),
            "archive must contain mpv.exe"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
