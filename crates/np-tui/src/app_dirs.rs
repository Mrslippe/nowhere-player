//! 应用目录只有这一处计算，main 与 launcher 共用。

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static WRITE_PROBE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub data: PathBuf,
    pub first_run: bool,
    /// 原首选目录不可写时的来源；用于日志/状态提示。
    pub fallback_from: Option<PathBuf>,
}

pub fn app_dirs() -> AppDirs {
    if let Some(base) = std::env::var_os("NP_TUI_DATA_DIR") {
        let base = PathBuf::from(base);
        return AppDirs {
            config: base.join("config"),
            cache: base.join("cache"),
            data: base.clone(),
            first_run: !base.is_dir(),
            fallback_from: None,
        };
    }
    let preferred = app_dirs_from_env(
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        std::env::consts::OS,
    );
    if can_write(&preferred.config) {
        return preferred;
    }

    // AppData 被 ACL / 漫游重定向 / 安全软件锁住时，不要要求用户管理员运行。
    // 依次退到 LOCALAPPDATA、TEMP；任何一处可写都比“配置永远保存失败”好。
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        candidates.push(app_dirs_from_env(
            Some(local),
            std::env::var_os("HOME").map(PathBuf::from),
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
            std::env::consts::OS,
        ));
    }
    let temp = std::env::temp_dir();
    candidates.push(app_dirs_from_env(
        Some(temp.clone()),
        Some(temp.clone()),
        Some(temp.clone()),
        Some(temp.join("cache")),
        std::env::consts::OS,
    ));
    for mut candidate in candidates {
        if can_write(&candidate.config) {
            candidate.fallback_from = Some(preferred.config.clone());
            return candidate;
        }
    }
    preferred
}

/// 目录可写性探测：能创建目录、能创建/删除探针文件，且 settings.json 不是只读。
fn can_write(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let settings = dir.join("settings.json");
    if let Ok(metadata) = fs::metadata(&settings)
        && metadata.permissions().readonly()
    {
        return false;
    }
    // settings.json 本身可能被 ACL 锁住：能创建目录不代表能覆盖这个文件。
    if settings.exists() && OpenOptions::new().write(true).open(&settings).is_err() {
        return false;
    }
    let sequence = WRITE_PROBE.fetch_add(1, Ordering::Relaxed);
    let probe = dir.join(format!(".np-write-probe-{}-{sequence}", std::process::id()));
    let result = OpenOptions::new().write(true).create_new(true).open(&probe);
    let writable = result.is_ok();
    drop(result);
    let _ = fs::remove_file(&probe);
    writable
}

/// 配置目录回退时，把可读的旧状态复制到新目录；不覆盖已存在文件。
///
/// 只复制用户状态，不复制 `cache/`，避免把大缓存搬进 TEMP。
pub fn copy_fallback_state(from: &Path, to: &Path) {
    let _ = fs::create_dir_all(to);
    for name in [
        "settings.json",
        "settings.bad.json",
        "live_history.json",
        "library.db",
        "library.db-wal",
        "library.db-shm",
        "bili-auth.bin",
    ] {
        copy_file_if_missing(&from.join(name), &to.join(name));
    }
    copy_dir_if_missing(&from.join("themes"), &to.join("themes"));
}

fn copy_file_if_missing(from: &Path, to: &Path) {
    if !from.is_file() || to.exists() {
        return;
    }
    let _ = fs::copy(from, to);
}

fn copy_dir_if_missing(from: &Path, to: &Path) {
    if !from.is_dir() || to.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    let _ = fs::create_dir_all(to);
    for entry in entries.flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir_if_missing(&source, &target);
        } else {
            copy_file_if_missing(&source, &target);
        }
    }
}

pub fn app_dirs_from_env(
    appdata: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_config: Option<PathBuf>,
    xdg_cache: Option<PathBuf>,
    os: &str,
) -> AppDirs {
    let (config, cache) = match os {
        "windows" => {
            let base = appdata
                .unwrap_or_else(|| PathBuf::from("."))
                .join("com.nowhere.player");
            (base.clone(), base.join("cache"))
        }
        "macos" => {
            let base = home
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library/Application Support/com.nowhere.player");
            (base.clone(), base.join("cache"))
        }
        _ => {
            let config = xdg_config
                .unwrap_or_else(|| {
                    home.clone()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".config")
                })
                .join("com.nowhere.player");
            let cache = xdg_cache
                .unwrap_or_else(|| home.unwrap_or_else(|| PathBuf::from(".")).join(".cache"))
                .join("com.nowhere.player");
            (config, cache)
        }
    };
    let first_run = !config.is_dir();
    AppDirs {
        data: config.clone(),
        config,
        cache,
        first_run,
        fallback_from: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_uses_roaming_appdata_com_nowhere_player() {
        let dirs = app_dirs_from_env(
            Some(PathBuf::from("C:/Users/x/AppData/Roaming")),
            None,
            None,
            None,
            "windows",
        );
        assert_eq!(
            dirs.config,
            PathBuf::from("C:/Users/x/AppData/Roaming/com.nowhere.player")
        );
        assert_eq!(dirs.data, dirs.config);
    }

    #[test]
    #[allow(clippy::permissions_set_readonly_false)]
    fn write_probe_rejects_a_read_only_settings_file() {
        let dir = std::env::temp_dir().join(format!(
            "np-appdirs-probe-{}-{}",
            std::process::id(),
            WRITE_PROBE.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(can_write(&dir), "fresh temp dir should be writable");
        fs::create_dir_all(&dir).expect("mkdir");
        let settings = dir.join("settings.json");
        fs::write(&settings, "{}").expect("write settings");
        let mut permissions = fs::metadata(&settings).expect("metadata").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&settings, permissions).expect("readonly");
        assert!(!can_write(&dir), "read-only settings must fall back");
        let mut permissions = fs::metadata(&settings).expect("metadata").permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(&settings, permissions);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fallback_copy_preserves_settings_and_themes() {
        let root = std::env::temp_dir().join(format!(
            "np-appdirs-copy-{}-{}",
            std::process::id(),
            WRITE_PROBE.fetch_add(1, Ordering::Relaxed)
        ));
        let source = root.join("old");
        let target = root.join("new");
        fs::create_dir_all(source.join("themes")).expect("mkdir");
        fs::write(source.join("settings.json"), "{\"theme.id\":\"x\"}").expect("settings");
        fs::write(source.join("themes/custom.json"), "{}").expect("theme");
        copy_fallback_state(&source, &target);
        assert_eq!(
            fs::read_to_string(target.join("settings.json")).expect("copied settings"),
            "{\"theme.id\":\"x\"}"
        );
        assert!(target.join("themes/custom.json").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn linux_uses_xdg_config_and_cache() {
        let dirs = app_dirs_from_env(
            None,
            Some(PathBuf::from("/home/x")),
            Some(PathBuf::from("/cfg")),
            Some(PathBuf::from("/cache")),
            "linux",
        );
        assert_eq!(dirs.config, PathBuf::from("/cfg/com.nowhere.player"));
        assert_eq!(dirs.cache, PathBuf::from("/cache/com.nowhere.player"));
    }
}
