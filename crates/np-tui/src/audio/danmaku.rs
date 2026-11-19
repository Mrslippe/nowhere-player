//! 弹幕：登录态读取、长连接、发送与轮询。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::danmaku_overlay::push_recent;
use crate::event_loop::{DanmakuLink, run_danmaku_with_retry, sleep_cancellable};

use super::live::DanmakuSession;
use super::{DanmakuReply, TuiAudio};

impl TuiAudio {
    /// 当前登录账号的 uid；未登录或凭证缺 `DedeUserID` 时为 `None`。
    ///
    /// 只读已经加载的登录态，不在绘制路径上读文件；未加载时不会误判自己的回显。
    pub(crate) fn own_bili_uid(&self) -> Option<u64> {
        self.bili_auth
            .borrow()
            .as_ref()
            .and_then(np_live::BiliAuth::uid)
    }

    /// 后台线程发送一条弹幕；返回本次请求的 generation。
    ///
    /// 不阻塞 UI，也不打印 cookie / csrf / 请求体；结果由
    /// [`poll_live_danmaku`](Self::poll_live_danmaku) 在主线程收取。
    pub(crate) fn send_live_danmaku(&self, message: &str) -> Result<u64, String> {
        if !self.is_live() {
            return Err("当前没有直播".to_owned());
        }
        let room_id = self.live_room.get();
        if room_id == 0 {
            return Err("房间号无效".to_owned());
        }
        if self.chat_rx.borrow().is_some() {
            return Err("弹幕正在发送中".to_owned());
        }
        let Some(auth) = self.auth_for_live() else {
            return Err("未登录，请先 /login bili".to_owned());
        };
        let generation = self.chat_generation.get().wrapping_add(1);
        self.chat_generation.set(generation);
        let (tx, rx) = std::sync::mpsc::channel();
        *self.chat_rx.borrow_mut() = Some(rx);
        #[cfg(test)]
        {
            self.chat_test_calls
                .set(self.chat_test_calls.get().saturating_add(1));
            if self.chat_test_result.borrow().is_some() {
                let result = self
                    .chat_test_result
                    .borrow_mut()
                    .take()
                    .expect("test danmaku result");
                let _ = tx.send((generation, result));
                return Ok(generation);
            }
        }
        let message = message.to_owned();
        let spawn = std::thread::Builder::new()
            .name("np-live-danmaku".to_owned())
            .spawn(move || {
                let result = match np_sources::ReqwestTransport::new() {
                    Ok(transport) => {
                        np_live::danmaku::send_danmaku(&transport, &auth, room_id, &message)
                            .map_err(|error| error.to_string())
                    }
                    Err(_) => Err("发送弹幕网络请求失败".to_owned()),
                };
                let _ = tx.send((generation, result));
            });
        if let Err(error) = spawn {
            *self.chat_rx.borrow_mut() = None;
            return Err(format!("弹幕发送线程启动失败：{error}"));
        }
        Ok(generation)
    }

    /// 主线程每 tick 调用：收取后台发送结果；旧 generation（换直播间 / 停播）丢弃。
    pub(crate) fn poll_live_danmaku(&self) -> Option<DanmakuReply> {
        let received = {
            let receiver = self.chat_rx.borrow();
            match receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(value) => Some(value),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                        self.chat_generation.get(),
                        Err("弹幕发送线程意外结束".to_owned()),
                    )),
                },
                None => None,
            }
        };
        let (generation, result) = received?;
        *self.chat_rx.borrow_mut() = None;
        if generation != self.chat_generation.get() {
            return None;
        }
        Some((generation, result))
    }

    /// 当前弹幕连接状态；Live 页状态栏只读它做轻提示。
    pub(crate) fn danmaku_link(&self) -> DanmakuLink {
        self.danmaku_link.get()
    }

    /// 主线程每 tick 调用：把弹幕消息收进环形缓冲，并更新连接状态。
    ///
    /// 返回 `true` 表示缓冲区或状态栏有变化，需要重绘。单条解析失败在
    /// np-live 层已经跳过，这里只面对结构化的 [`np_live::DanmakuMessage`]。
    pub(crate) fn poll_danmaku(&self) -> bool {
        let mut changed = false;
        let mut received = Vec::new();
        let mut links = Vec::new();
        let mut messages_done = false;
        let mut links_done = false;
        {
            let session = self.danmaku_session.borrow();
            let Some(session) = session.as_ref() else {
                return false;
            };
            if session.generation != self.danmaku_generation.get() {
                return false;
            }
            loop {
                match session.messages.try_recv() {
                    Ok(message) => received.push(message),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        messages_done = true;
                        break;
                    }
                }
            }
            loop {
                match session.links.try_recv() {
                    Ok(link) => links.push(link),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        links_done = true;
                        break;
                    }
                }
            }
        }

        if !received.is_empty() {
            changed = true;
            let added = u64::try_from(received.len()).unwrap_or(u64::MAX);
            let mut recent = self.danmaku_recent.borrow_mut();
            for message in received {
                push_recent(&mut recent, message);
            }
            self.danmaku_received
                .set(self.danmaku_received.get().saturating_add(added));
            // 连接成功后不一定马上有弹幕；收到第一条就足以确认链路可用。
            if self.danmaku_link.get() != DanmakuLink::Failed
                && self.danmaku_link.get() != DanmakuLink::Connected
            {
                self.danmaku_link.set(DanmakuLink::Connected);
                changed = true;
            }
        }

        for link in links {
            if link != self.danmaku_link.get() {
                self.danmaku_link.set(link);
                changed = true;
            }
        }

        // 线程退出后两侧通道都会断开；保留缓冲和最终状态，只释放通道。
        if messages_done && links_done {
            if matches!(
                self.danmaku_link.get(),
                DanmakuLink::Connecting | DanmakuLink::Reconnecting { .. }
            ) {
                // 线程异常结束（没有发最终状态）时不要把“连接中”挂死。
                self.danmaku_link.set(DanmakuLink::Failed);
                changed = true;
            }
            *self.danmaku_session.borrow_mut() = None;
            *self.danmaku_cancel.borrow_mut() = None;
        }
        changed
    }

    /// 启动弹幕长连接：断线按 1/2/5/10/20/30s 重连，最多 6 次。
    ///
    /// 每次重连都会重新走 `getDanmuInfo`；日志与状态文本不包含 token / cookie。
    pub(super) fn start_danmaku(&self, room_id: u64, auth: Option<np_live::BiliAuth>) {
        self.stop_danmaku();
        let generation = self.danmaku_generation.get().wrapping_add(1);
        self.danmaku_generation.set(generation);
        self.danmaku_recent.borrow_mut().clear();
        self.danmaku_link.set(DanmakuLink::Connecting);

        let cancel = Arc::new(AtomicBool::new(false));
        *self.danmaku_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (message_tx, message_rx) = std::sync::mpsc::channel();
        let (link_tx, link_rx) = std::sync::mpsc::channel();
        *self.danmaku_session.borrow_mut() = Some(DanmakuSession {
            generation,
            messages: message_rx,
            links: link_rx,
        });

        let spawn = std::thread::Builder::new()
            .name("np-live-danmaku".to_owned())
            .spawn(move || {
                let transport = np_sources::ReqwestTransport::new();
                let transport = transport.as_ref();
                let _ = run_danmaku_with_retry(
                    &cancel,
                    || match transport {
                        Ok(transport) => np_live::run_danmaku_ws(
                            transport,
                            room_id,
                            auth.as_ref(),
                            &cancel,
                            &message_tx,
                        ),
                        Err(_) => Err(np_live::DanmakuWsError::Connect(
                            "网络客户端初始化失败".to_owned(),
                        )),
                    },
                    |delay| sleep_cancellable(&cancel, delay),
                    |link| {
                        let _ = link_tx.send(link);
                    },
                );
            });
        if let Err(error) = spawn {
            self.stop_danmaku();
            self.danmaku_link.set(DanmakuLink::Failed);
            *self.live_note.borrow_mut() = Some(format!("弹幕连接线程启动失败：{error}"));
        }
    }

    /// cancel 并丢弃当前弹幕会话；generation 递增，旧线程的回传全部作废。
    pub(super) fn stop_danmaku(&self) {
        if let Some(cancel) = self.danmaku_cancel.borrow_mut().take() {
            cancel.store(true, Ordering::Release);
        }
        self.danmaku_generation
            .set(self.danmaku_generation.get().wrapping_add(1));
        *self.danmaku_session.borrow_mut() = None;
        self.danmaku_recent.borrow_mut().clear();
        self.danmaku_link.set(DanmakuLink::Idle);
    }
}
