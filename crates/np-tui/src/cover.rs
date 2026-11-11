//! 封面：ratatui-image + ThreadProtocol，编码在工作线程里做。

use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use ratatui::layout::{Rect, Size};
use ratatui::widgets::Widget;
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::{Resize, StatefulImage};

pub struct CoverView {
    state: Option<ThreadProtocol>,
    // Worker responses are drained on the UI thread.
    rx: Option<Receiver<ResizeResponse>>,
    last_area: Size,
    pub protocol: String,
}

impl CoverView {
    pub fn empty() -> Self {
        Self {
            state: None,
            rx: None,
            last_area: Size::default(),
            protocol: "无封面".to_owned(),
        }
    }

    pub fn load(path: &Path) -> Self {
        let Ok(picker) = Picker::from_query_stdio() else {
            return Self::empty();
        };
        let Ok(reader) = image::ImageReader::open(path) else {
            return Self::empty();
        };
        let Ok(image) = reader.decode() else {
            return Self::empty();
        };
        let protocol = format!("{:?}", picker.protocol_type());
        let (tx_request, rx_request) = mpsc::channel::<ResizeRequest>();
        let (tx_response, rx_response) = mpsc::channel::<ResizeResponse>();
        std::thread::spawn(move || {
            while let Ok(request) = rx_request.recv() {
                if let Ok(response) = request.resize_encode() {
                    let _ = tx_response.send(response);
                }
            }
        });
        Self {
            state: Some(ThreadProtocol::new(
                tx_request,
                Some(picker.new_resize_protocol(image)),
            )),
            rx: Some(rx_response),
            last_area: Size::default(),
            protocol,
        }
    }

    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// 主动收工作线程的编码结果；返回是否有更新。
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            return false;
        };
        let mut updated = false;
        while let Ok(response) = rx.try_recv() {
            if let Some(state) = &mut self.state {
                let _ = state.update_resized_protocol(response);
                updated = true;
            }
        }
        updated
    }

    pub fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        if area.width < 2 || area.height < 2 {
            return;
        }
        if self.state.is_none() {
            let placeholder = ratatui::widgets::Paragraph::new(format!(
                "{} {} {}\n{} {} {}\n{} {} {}",
                crate::glyphs::COVER_FILL,
                crate::glyphs::COVER_FILL,
                crate::glyphs::COVER_FILL,
                crate::glyphs::COVER_FILL,
                crate::glyphs::NOTE,
                crate::glyphs::COVER_FILL,
                crate::glyphs::COVER_FILL,
                crate::glyphs::COVER_FILL,
                crate::glyphs::COVER_FILL,
            ))
            .style(ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray));
            let inner = Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: area.height.saturating_sub(1),
            };
            placeholder.render(inner, frame.buffer_mut());
            return;
        }

        if area.as_size() != self.last_area {
            self.last_area = area.as_size();
        }
        let state = self.state.as_mut().expect("state");
        frame.render_stateful_widget(StatefulImage::new().resize(Resize::Fit(None)), area, state);
    }

    /// 给 `ThreadProtocol` 的工作线程发送尺寸请求；由 `StatefulImage` 在
    /// render 时自动触发，这里保留方法方便测试/显式预热。
    #[allow(dead_code)]
    pub fn size(&self) -> Size {
        self.last_area
    }
}
