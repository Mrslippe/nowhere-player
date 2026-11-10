//! 设置存储（ADR-0005）：一份 `<config_dir>/settings.json`，点分 key，
//! 值只有字符串 / 数字 / 布尔三种。
//!
//! 本模块只做纯逻辑：不依赖 Tauri、播放器或音源；坏文件回落到默认并备份
//! `settings.bad.json`，让启动永远不被一份坏配置卡住。

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 设置文件名。
pub const SETTINGS_FILE: &str = "settings.json";
/// 设置坏文件的备份名。
pub const SETTINGS_BACKUP_FILE: &str = "settings.bad.json";

/// 一个设置值：只允许字符串 / 数字 / 布尔（不允许嵌套对象，见 ADR-0005 §2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    /// 布尔。
    Bool(bool),
    /// 数字（JSON number）。
    Num(f64),
    /// 字符串。
    Str(String),
}

impl SettingValue {
    /// `config set` 的类型推断：`true|false` → 布尔；纯数字 → 数字；其余 → 字符串。
    pub fn parse(raw: &str) -> Self {
        match raw {
            "true" => Self::Bool(true),
            "false" => Self::Bool(false),
            _ => raw
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map_or_else(|| Self::Str(raw.to_owned()), Self::Num),
        }
    }

    /// 字符串取值。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    /// 布尔取值。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// 终端展示形式：`true` / `false` / 数字 / 原字符串。
    pub fn display(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Num(value) => {
                let mut text = value.to_string();
                if text.ends_with(".0") {
                    text.truncate(text.len() - 2);
                }
                text
            }
            Self::Str(value) => value.clone(),
        }
    }
}

/// 一次 `load` 产生的非致命问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsWarning {
    /// 面向用户的说明。
    pub message: String,
}

/// 设置读写错误（IO / 序列化）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    /// 面向用户的说明。
    pub message: String,
}

impl SettingsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SettingsError {}

/// 一份设置：key → 值，按 key 排序（序列化后也稳定）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Settings {
    values: BTreeMap<String, SettingValue>,
}

impl Settings {
    /// 空设置（所有 key 走内置默认）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `<dir>/settings.json` 读取；文件不存在视为空。
    ///
    /// JSON 损坏时备份为 `settings.bad.json` 并返回默认 + 警告，绝不阻塞启动。
    pub fn load(dir: &Path) -> (Self, Vec<SettingsWarning>) {
        let path = dir.join(SETTINGS_FILE);
        if !path.exists() {
            return (Self::new(), Vec::new());
        }

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                return (
                    Self::new(),
                    vec![SettingsWarning {
                        message: format!("无法读取设置文件（{}）：{error}", path.display()),
                    }],
                );
            }
        };

        match serde_json::from_str::<Self>(&text) {
            Ok(settings) => (settings, Vec::new()),
            Err(error) => {
                let backup = dir.join(SETTINGS_BACKUP_FILE);
                let backup_note = match fs::copy(&path, &backup) {
                    Ok(_) => format!("，已备份为 {}", backup.display()),
                    Err(backup_error) => format!("，备份失败：{backup_error}"),
                };
                (
                    Self::new(),
                    vec![SettingsWarning {
                        message: format!("设置文件损坏（{error}）{backup_note}"),
                    }],
                )
            }
        }
    }

    /// 原子写：先写 `settings.json.tmp`，再 rename 到 `settings.json`。
    pub fn save(&self, dir: &Path) -> Result<(), SettingsError> {
        fs::create_dir_all(dir).map_err(|error| {
            SettingsError::new(format!("无法创建设置目录 {}：{error}", dir.display()))
        })?;

        let path = dir.join(SETTINGS_FILE);
        let tmp = dir.join(format!("{SETTINGS_FILE}.tmp"));
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| SettingsError::new(format!("设置序列化失败：{error}")))?;

        if let Err(error) = fs::write(&tmp, json) {
            let _ = fs::remove_file(&tmp);
            return Err(SettingsError::new(format!("写入临时设置文件失败：{error}")));
        }
        if let Err(error) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(SettingsError::new(format!("替换设置文件失败：{error}")));
        }
        Ok(())
    }

    /// 读取一个 key。
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }

    /// 写入一个 key（调用方负责受控 key 的取值校验）。
    pub fn set(&mut self, key: impl Into<String>, value: SettingValue) {
        self.values.insert(key.into(), value);
    }

    /// 删除一个 key，返回原值。
    pub fn unset(&mut self, key: &str) -> Option<SettingValue> {
        self.values.remove(key)
    }

    /// 清空全部设置（恢复默认）。
    pub fn reset(&mut self) {
        self.values.clear();
    }

    /// 按 key 升序遍历。
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SettingValue)> {
        self.values.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// key 数量。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// 受控 key 的取值校验；未知 key 允许写入（ADR-0005 §2）。
///
/// - `theme.id`：必须是字符串，且存在于 `known_themes`；
/// - `source.<id>.enabled`：`<id>` 必须存在于 `known_sources`，值必须是布尔。
pub fn validate_setting(
    key: &str,
    value: &SettingValue,
    known_themes: &[String],
    known_sources: &[&str],
) -> Result<(), String> {
    if key == "theme.id" {
        let Some(id) = value.as_str() else {
            return Err("theme.id 必须是字符串".to_owned());
        };
        if !known_themes.iter().any(|theme| theme == id) {
            return Err(format!(
                "主题不存在：{id}（可用：{}）",
                known_themes.join(" / ")
            ));
        }
        return Ok(());
    }

    if let Some(source_id) = key
        .strip_prefix("source.")
        .and_then(|rest| rest.strip_suffix(".enabled"))
    {
        if !known_sources.contains(&source_id) {
            return Err(format!("未知音源：{source_id}"));
        }
        if value.as_bool().is_none() {
            return Err(format!("{key} 必须是 true 或 false"));
        }
    }

    Ok(())
}

/// 设置文件路径（便于命令层展示）。
pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join(SETTINGS_FILE)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = format!(
                "np-settings-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn missing_file_loads_empty_without_warning() {
        let dir = TempDir::new("missing");
        let (settings, warnings) = Settings::load(dir.path());
        assert!(settings.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn save_and_load_round_trip_is_sorted() {
        let dir = TempDir::new("roundtrip");
        let mut settings = Settings::new();
        settings.set("theme.id", SettingValue::Str("ice".into()));
        settings.set("source.bilibili.enabled", SettingValue::Bool(false));
        settings.save(dir.path()).expect("save");

        let text = fs::read_to_string(dir.path().join(SETTINGS_FILE)).expect("read");
        let theme = text.find("\"theme.id\"").expect("theme");
        let source = text.find("\"source.").expect("source");
        assert!(source < theme, "JSON keys must be sorted");

        let (loaded, warnings) = Settings::load(dir.path());
        assert!(warnings.is_empty());
        assert_eq!(loaded, settings);
        assert_eq!(
            loaded.get("theme.id").and_then(SettingValue::as_str),
            Some("ice")
        );
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaulted() {
        let dir = TempDir::new("bad");
        fs::write(dir.path().join(SETTINGS_FILE), "{not json").expect("write bad");

        let (settings, warnings) = Settings::load(dir.path());

        assert!(settings.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("已备份"));
        assert!(dir.path().join(SETTINGS_BACKUP_FILE).is_file());
    }

    #[test]
    fn type_inference_parses_bool_numbers_and_strings() {
        assert_eq!(SettingValue::parse("true"), SettingValue::Bool(true));
        assert_eq!(SettingValue::parse("false"), SettingValue::Bool(false));
        assert_eq!(SettingValue::parse("300"), SettingValue::Num(300.0));
        assert_eq!(SettingValue::parse("-1.5"), SettingValue::Num(-1.5));
        assert_eq!(
            SettingValue::parse("ice"),
            SettingValue::Str("ice".to_owned())
        );
        assert_eq!(
            SettingValue::parse("NaN"),
            SettingValue::Str("NaN".to_owned())
        );
    }

    #[test]
    fn get_set_unset_reset() {
        let mut settings = Settings::new();
        settings.set("a", SettingValue::Num(1.0));
        assert_eq!(settings.get("a"), Some(&SettingValue::Num(1.0)));
        assert_eq!(settings.unset("a"), Some(SettingValue::Num(1.0)));
        assert!(settings.is_empty());
        settings.set("b", SettingValue::Bool(true));
        settings.reset();
        assert!(settings.is_empty());
    }

    #[test]
    fn display_formats_values_for_terminal() {
        assert_eq!(SettingValue::Num(300.0).display(), "300");
        assert_eq!(SettingValue::Num(1.5).display(), "1.5");
        assert_eq!(SettingValue::Bool(false).display(), "false");
        assert_eq!(SettingValue::Str("x".into()).display(), "x");
    }

    #[test]
    fn theme_id_validation_requires_known_theme() {
        let themes = vec!["graphite".to_owned(), "ice".to_owned()];
        assert!(
            validate_setting("theme.id", &SettingValue::Str("ice".into()), &themes, &[]).is_ok()
        );
        assert!(
            validate_setting("theme.id", &SettingValue::Str("nope".into()), &themes, &[]).is_err()
        );
        assert!(validate_setting("theme.id", &SettingValue::Bool(true), &themes, &[]).is_err());
    }

    #[test]
    fn source_enabled_validation_requires_known_id_and_bool() {
        let sources = ["bilibili", "netease"];
        assert!(
            validate_setting(
                "source.bilibili.enabled",
                &SettingValue::Bool(false),
                &[],
                &sources
            )
            .is_ok()
        );
        assert!(
            validate_setting(
                "source.youtube.enabled",
                &SettingValue::Bool(false),
                &[],
                &sources
            )
            .is_err()
        );
        assert!(
            validate_setting(
                "source.bilibili.enabled",
                &SettingValue::Str("no".into()),
                &[],
                &sources
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_key_is_allowed() {
        assert!(
            validate_setting("playback.crossfadeMs", &SettingValue::Num(300.0), &[], &[]).is_ok()
        );
        assert!(
            validate_setting("anything.goes", &SettingValue::Str("x".into()), &[], &[]).is_ok()
        );
    }

    #[test]
    fn save_leaves_no_tmp_file() {
        let dir = TempDir::new("tmp");
        Settings::new().save(dir.path()).expect("save");
        assert!(!dir.path().join(format!("{SETTINGS_FILE}.tmp")).exists());
        assert!(dir.path().join(SETTINGS_FILE).is_file());
    }

    #[test]
    fn bad_file_load_does_not_overwrite_original() {
        let dir = TempDir::new("bad-keep");
        let path = dir.path().join(SETTINGS_FILE);
        fs::write(&path, "not-json").expect("write");
        let _ = Settings::load(dir.path());
        assert_eq!(fs::read_to_string(&path).expect("read"), "not-json");
    }
}
