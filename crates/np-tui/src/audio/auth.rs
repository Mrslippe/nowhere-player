//! 扫码登录与登录态凭证。

use super::TuiAudio;

impl TuiAudio {
    pub(crate) fn start_bili_login(&self) {
        self.login.borrow_mut().start();
    }

    pub(crate) fn start_bili_logout(&self) -> String {
        let note = self.login.borrow_mut().logout();
        *self.bili_auth.borrow_mut() = None;
        self.auth_loaded.set(true);
        note
    }

    pub(crate) fn login_active(&self) -> bool {
        self.login.borrow().active()
    }

    pub(crate) fn poll_login(&self) -> bool {
        let changed = self.login.borrow_mut().poll();
        if let Some(auth) = self.login.borrow().auth() {
            *self.bili_auth.borrow_mut() = Some(auth);
            self.auth_loaded.set(true);
        }
        changed
    }

    pub(crate) fn handle_login_key(&self, key: crossterm::event::KeyCode) -> bool {
        self.login.borrow_mut().handle_key(key)
    }

    pub(crate) fn draw_login(
        &self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        palette: &crate::theme::Palette,
    ) {
        self.login.borrow().draw(frame, area, palette);
    }

    pub(super) fn auth_for_live(&self) -> Option<np_live::BiliAuth> {
        if !self.auth_loaded.get() {
            self.auth_loaded.set(true);
            match np_live::AuthStore::new(crate::login::auth_path()).load() {
                Ok(Some(auth)) => *self.bili_auth.borrow_mut() = Some(auth),
                Ok(None) => {}
                Err(error) => {
                    *self.live_note.borrow_mut() = Some(format!("B站登录态读取失败：{error}"));
                }
            }
        }
        self.bili_auth.borrow().clone()
    }

    /// 登录态是否具备发弹幕所需的两块凭证；不向调用方暴露 cookie 内容。
    pub(crate) fn has_bili_auth(&self) -> bool {
        self.auth_for_live().is_some_and(|auth| {
            auth.cookie("SESSDATA").is_some() && auth.cookie("bili_jct").is_some()
        })
    }
}
