//! B站扫码登录 UI/后台轮询（T-0092）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Clear, Paragraph};

use np_live::auth::{AuthStore, BiliAuth, QR_POLL_INTERVAL, QrStatus, generate_qr, poll_qr};
use np_sources::ReqwestTransport;

use crate::theme::{Palette, PaletteToken};

enum LoginEvent {
    Qr(Vec<String>),
    Status(String),
    Success(BiliAuth, Option<String>),
    Error(String),
}

pub(crate) struct LoginUi {
    phase: Phase,
    qr_lines: Vec<String>,
    status: String,
    rx: Option<mpsc::Receiver<LoginEvent>>,
    cancel: Option<Arc<AtomicBool>>,
    auth: Option<BiliAuth>,
}

#[derive(PartialEq, Eq)]
enum Phase {
    Idle,
    Starting,
    Waiting,
    Done,
}

pub(crate) fn auth_path() -> std::path::PathBuf {
    crate::app_dirs::app_dirs().data.join("bili-auth.bin")
}

impl LoginUi {
    pub(crate) fn new() -> Self {
        Self {
            phase: Phase::Idle,
            qr_lines: Vec::new(),
            status: String::new(),
            rx: None,
            cancel: None,
            auth: None,
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.phase != Phase::Idle
    }

    pub(crate) fn auth(&self) -> Option<BiliAuth> {
        self.auth.clone()
    }

    pub(crate) fn start(&mut self) {
        self.cancel_current();
        self.phase = Phase::Starting;
        self.qr_lines.clear();
        self.status = "正在生成二维码…".to_owned();
        self.auth = None;

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(Arc::clone(&cancel));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let path = auth_path();
        std::thread::Builder::new()
            .name("np-bili-login".to_owned())
            .spawn(move || login_worker(tx, cancel, path))
            .ok();
    }

    pub(crate) fn logout(&mut self) -> String {
        self.cancel_current();
        self.phase = Phase::Idle;
        self.qr_lines.clear();
        self.auth = None;
        match AuthStore::new(auth_path()).clear() {
            Ok(()) => "B站登录已清除".to_owned(),
            Err(error) => format!("B站登录已清除，但删除凭证失败：{error}"),
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(rx) = self.rx.as_ref() {
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            };
            changed = true;
            match event {
                LoginEvent::Qr(lines) => {
                    self.qr_lines = lines;
                    self.phase = Phase::Waiting;
                    self.status = "用哔哩哔哩 App 扫码".to_owned();
                }
                LoginEvent::Status(status) => {
                    self.status = status;
                }
                LoginEvent::Success(auth, save_error) => {
                    self.auth = Some(auth);
                    self.phase = Phase::Done;
                    self.rx = None;
                    self.status = match save_error {
                        Some(error) => format!("登录成功，但保存凭证失败：{error}"),
                        None => "B站登录成功".to_owned(),
                    };
                }
                LoginEvent::Error(error) => {
                    self.phase = Phase::Done;
                    self.rx = None;
                    self.status = error;
                }
            }
        }
        changed
    }

    pub(crate) fn handle_key(&mut self, key: KeyCode) -> bool {
        if !self.active() {
            return false;
        }
        match key {
            KeyCode::Esc => {
                self.cancel_current();
                self.phase = Phase::Idle;
                self.qr_lines.clear();
                self.status = "已取消 B站登录".to_owned();
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.start();
                true
            }
            _ => true,
        }
    }

    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        if !self.active() {
            return;
        }
        frame.render_widget(Clear, area);
        let bg = palette.color(PaletteToken::Canvas);
        let status = if self.status.is_empty() {
            "B站登录 · Esc 取消 · R 刷新".to_owned()
        } else {
            format!("{} · Esc 取消 · R 刷新", self.status)
        };
        let status_y = area.bottom().saturating_sub(1);
        frame.render_widget(
            Paragraph::new(status).style(
                Style::default()
                    .fg(palette.color(PaletteToken::Muted))
                    .bg(bg),
            ),
            Rect {
                x: area.x,
                y: status_y,
                width: area.width,
                height: 1,
            },
        );
        if self.qr_lines.is_empty() {
            frame.render_widget(
                Paragraph::new("正在生成二维码…")
                    .alignment(ratatui::layout::Alignment::Center)
                    .style(
                        Style::default()
                            .fg(palette.color(PaletteToken::Text))
                            .bg(bg),
                    ),
                area,
            );
            return;
        }
        let qr_height = u16::try_from(self.qr_lines.len()).unwrap_or(u16::MAX);
        if qr_height.saturating_add(1) > area.height {
            frame.render_widget(
                Paragraph::new("终端高度不足，无法显示二维码")
                    .alignment(ratatui::layout::Alignment::Center)
                    .style(
                        Style::default()
                            .fg(palette.color(PaletteToken::Accent))
                            .bg(bg),
                    ),
                area,
            );
            return;
        }
        let qr_width = self
            .qr_lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0) as u16;
        let x = area
            .x
            .saturating_add(area.width.saturating_sub(qr_width) / 2);
        let y = area
            .y
            .saturating_add(area.height.saturating_sub(qr_height + 1) / 2);
        frame.render_widget(
            Paragraph::new(self.qr_lines.join("\n")).style(
                Style::default()
                    .fg(palette.color(PaletteToken::Text))
                    .bg(bg),
            ),
            Rect {
                x,
                y,
                width: qr_width.min(area.width),
                height: qr_height,
            },
        );
    }

    fn cancel_current(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.rx = None;
    }
}

fn login_worker(tx: mpsc::Sender<LoginEvent>, cancel: Arc<AtomicBool>, path: std::path::PathBuf) {
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(_) => {
            let _ = tx.send(LoginEvent::Error("登录网络客户端初始化失败".to_owned()));
            return;
        }
    };
    let qr = match generate_qr(&transport) {
        Ok(qr) => qr,
        Err(error) => {
            let _ = tx.send(LoginEvent::Error(format!("二维码生成失败：{error}")));
            return;
        }
    };
    let lines = match crate::qr::render_qr(&qr.qr_url) {
        Ok(lines) => lines,
        Err(error) => {
            let _ = tx.send(LoginEvent::Error(format!("二维码渲染失败：{error}")));
            return;
        }
    };
    if tx.send(LoginEvent::Qr(lines)).is_err() {
        return;
    }
    loop {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match poll_qr(&transport, &qr.qrcode_key) {
            Ok(QrStatus::Waiting) => {
                let _ = tx.send(LoginEvent::Status("等待扫码".to_owned()));
            }
            Ok(QrStatus::Scanned) => {
                let _ = tx.send(LoginEvent::Status("已扫码，请在手机上确认".to_owned()));
            }
            Ok(QrStatus::Expired) => {
                let _ = tx.send(LoginEvent::Error("二维码已过期，按 R 刷新".to_owned()));
                return;
            }
            Ok(QrStatus::Success(auth)) => {
                let save_error = AuthStore::new(&path)
                    .save(&auth)
                    .err()
                    .map(|error| error.to_string());
                let _ = tx.send(LoginEvent::Success(auth, save_error));
                return;
            }
            Err(error) => {
                let _ = tx.send(LoginEvent::Error(format!("登录轮询失败：{error}")));
                return;
            }
        }
        for _ in 0..(QR_POLL_INTERVAL.as_millis() / 100) {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
