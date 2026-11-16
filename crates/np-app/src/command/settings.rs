use super::*;

pub struct SettingsHandle {
    dir: PathBuf,
    inner: Mutex<Settings>,
    warnings: Vec<SettingsWarning>,
}

impl SettingsHandle {
    pub fn load(dir: PathBuf) -> Self {
        let (settings, warnings) = Settings::load(&dir);
        Self {
            dir,
            inner: Mutex::new(settings),
            warnings,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn warnings(&self) -> &[SettingsWarning] {
        &self.warnings
    }

    pub fn get(&self, key: &str) -> Option<SettingValue> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key)
            .and_then(|value| value.as_str().map(str::to_owned))
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|value| value.as_bool())
    }

    pub fn theme_id(&self) -> Option<String> {
        self.get_str("theme.id")
    }

    pub fn entries(&self) -> Vec<(String, SettingValue)> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(key, value)| (key.to_owned(), value.clone()))
            .collect()
    }

    pub fn enabled_source_ids(&self, known: &[SourceId]) -> HashSet<SourceId> {
        known
            .iter()
            .copied()
            .filter(|source| {
                self.get_bool(&format!("source.{source}.enabled"))
                    .unwrap_or(true)
            })
            .collect()
    }

    pub fn set_and_save(
        &self,
        key: impl Into<String>,
        value: SettingValue,
    ) -> Result<(), CmdError> {
        let mut settings = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings.set(key, value);
        save_settings(&self.dir, &settings)
    }

    pub fn unset_and_save(&self, key: &str) -> Result<Option<SettingValue>, CmdError> {
        let mut settings = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = settings.unset(key);
        save_settings(&self.dir, &settings)?;
        Ok(removed)
    }

    pub fn reset_and_save(&self) -> Result<(), CmdError> {
        let mut settings = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings.reset();
        save_settings(&self.dir, &settings)
    }
}

fn save_settings(dir: &Path, settings: &Settings) -> Result<(), CmdError> {
    settings
        .save(dir)
        .map_err(|error| CmdError::new("E_SETTINGS_IO", error.to_string()))
}

/// 当前注册表支持的全部源（`source list` 在禁用状态下也要能列出来）。
pub const KNOWN_SOURCES: [SourceId; 2] = [SourceId::Bilibili, SourceId::Netease];

/// 四模式规范 id；与 TUI `playback.mode` 设置共用。
pub(crate) fn playback_mode_id(mode: PlaybackMode) -> &'static str {
    match (mode.repeat, mode.shuffle) {
        (RepeatMode::One, false) => "single",
        (RepeatMode::Off, true) => "shuffle",
        (RepeatMode::All, false) => "list",
        _ => "sequential",
    }
}
