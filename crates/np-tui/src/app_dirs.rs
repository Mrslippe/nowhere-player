//! 应用目录只有这一处计算，main 与 launcher 共用。

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub data: PathBuf,
    pub first_run: bool,
}

pub fn app_dirs() -> AppDirs {
    if let Some(base) = std::env::var_os("NP_TUI_DATA_DIR") {
        let base = PathBuf::from(base);
        return AppDirs {
            config: base.join("config"),
            cache: base.join("cache"),
            data: base.clone(),
            first_run: !base.is_dir(),
        };
    }
    app_dirs_from_env(
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        std::env::consts::OS,
    )
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
