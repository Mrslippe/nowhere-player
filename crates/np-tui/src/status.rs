//! 状态栏 TTL 与块状态文案：放在独立模块，避免 app.rs 超过行数上限。

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use np_core::{Block, BlockStatus};

use crate::app::App;

/// 成功 / 取消类状态在状态栏停留的时长。
pub(crate) const TRANSIENT_STATUS_TTL: Duration = Duration::from_secs(3);

impl App {
    /// 常驻状态：错误、搜索中、准备中、登录中都用它，不会被 tick 清掉。
    pub(crate) fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_expires_at = None;
    }

    /// 成功 / 取消类状态：3 秒后由事件循环 tick 清除。
    pub(crate) fn set_transient_status(&mut self, text: impl Into<String>) {
        self.set_transient_status_at(text, Instant::now());
    }

    /// 可测版本：显式传入 `now`，避免测试 sleep。
    pub(crate) fn set_transient_status_at(&mut self, text: impl Into<String>, now: Instant) {
        self.status = text.into();
        self.status_expires_at = Some(now + TRANSIENT_STATUS_TTL);
    }

    /// 到期把瞬时状态恢复成 `default`；返回是否发生变化。
    pub(crate) fn expire_status(&mut self, now: Instant, default: &str) -> bool {
        if self.status_expires_at.is_some_and(|at| now >= at) {
            self.status = default.to_owned();
            self.status_expires_at = None;
            return true;
        }
        false
    }

    /// 弹幕已发送等提示同样 3 秒后消失。
    pub(crate) fn show_chat_notice(&mut self, text: impl Into<String>) {
        self.show_chat_notice_at(text, Instant::now());
    }

    /// 可测版本：显式传入 `now`。
    pub(crate) fn show_chat_notice_at(&mut self, text: impl Into<String>, now: Instant) {
        self.chat_notice = Some(text.into());
        self.chat_notice_expires_at = Some(now + TRANSIENT_STATUS_TTL);
    }

    pub(crate) fn expire_chat_notice(&mut self, now: Instant) -> bool {
        if self.chat_notice_expires_at.is_some_and(|at| now >= at) {
            self.chat_notice = None;
            self.chat_notice_expires_at = None;
            return true;
        }
        false
    }
}

impl BlockStatusExt for Block {
    fn status_label(&self) -> &'static str {
        match self.status {
            BlockStatus::Pending => "等待",
            BlockStatus::Running => "执行中",
            BlockStatus::Done => "完成",
            BlockStatus::Error => "失败",
            BlockStatus::Cancelled => "取消",
        }
    }
}

pub(crate) trait BlockStatusExt {
    fn status_label(&self) -> &'static str;
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
