//! R10: grid 模型层(原 app/src/terminal/model/grid)。
//! 事件发送与富内容 secret 检测经缝注入,本 crate 不依赖 app。

pub mod displayed_output;
pub mod grid_handler;
pub mod header_grid;
mod grid_storage;
pub mod indexing;
pub mod selection;
mod selection_cursor;
pub mod storage;

pub mod alt_screen;
pub mod block_types;
pub mod blockgrid;
#[cfg(any(test, feature = "test-support"))]
pub mod mock_blockgrid;
pub mod color_sampler;
pub use block_types::{BlockGridPoint, BlockPadding, BlockSize, GridType, WithinBlock};
pub mod ansi_handler;
pub mod ansi_state;
pub mod image_geometry;
pub use image_geometry::{resize_dimensions, FitType};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollDelta {
    Up { lines: usize },
    Down { lines: usize },
}

impl ScrollDelta {
    /// Technically, `Self::Down { lines: 0 }` and `Self::Up { lines: 0 }` are equivalent.
    pub fn zero() -> Self {
        Self::Down { lines: 0 }
    }
}

impl Default for ScrollDelta {
    fn default() -> Self {
        ScrollDelta::zero()
    }
}

pub mod tab_stops;

pub mod grapheme_cursor;

pub use warp_terminal::model::grid::row;

pub use displayed_output::*;

enum CursorDirection {
    Up,
    Down,
    Left,
    Right,
}

enum CursorState {
    Valid,
    Exhausted(CursorDirection),
    Invalid,
}
pub use ansi_state::*;
pub use grid_handler::*;
pub use grid_storage::*;
pub use storage::Storage;
pub use indexing::{ConvertToAbsolute, IndexRegion};
pub use selection_cursor::SelectionCursor;

/// R10 事件缝: grid → app 事件循环。app 装配处实现本 trait 并包装
/// ChannelEventListener(原直依,见 grid_handler.rs:354)。
pub trait GridEventSink: Send + Sync + 'static {
    fn send_cursor_blinking_change(&self, blinking: bool);
    fn send_bell(&self);
    fn send_mouse_cursor_dirty(&self);
    fn send_clipboard_store(&self, kind: crate::ClipboardType, text: std::sync::Arc<str>);
    fn send_clipboard_load(
        &self,
        kind: crate::ClipboardType,
        respond: ClipboardRespondFn,
    );
    fn send_image_received(&self, image_id: u32, image_data: Vec<u8>, protocol: ImageProtocol);
    /// R13: alt_screen 下沉新增——选择变化通知(app 映射 TerminalEvent::TextSelectionChanged)。
    fn send_text_selection_changed(&self);
    /// R14: header_grid 下沉新增——PS1 honor 失同步通知(app 映射 TerminalEvent::HonorPS1OutOfSync)。
    fn send_honor_ps1_out_of_sync(&self);
}

/// clipboard 回读回调: 入参为系统剪贴板文本,返回需写回 pty 的响应串。
pub type ClipboardRespondFn = std::sync::Arc<dyn Fn(String) -> String + Send + Sync>;

/// R10: 图像协议枚举(crate 本地)。app adapter 映射 telemetry::ImageProtocol。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProtocol {
    Kitty,
    ITerm,
}

/// 测试用 no-op 事件汇。
pub struct NoopEventSink;
impl GridEventSink for NoopEventSink {
    fn send_cursor_blinking_change(&self, _blinking: bool) {}
    fn send_bell(&self) {}
    fn send_mouse_cursor_dirty(&self) {}
    fn send_clipboard_store(&self, _kind: crate::ClipboardType, _text: std::sync::Arc<str>) {}
    fn send_clipboard_load(&self, _kind: crate::ClipboardType, _respond: ClipboardRespondFn) {}
    fn send_image_received(&self, _id: u32, _data: Vec<u8>, _protocol: ImageProtocol) {}
    fn send_text_selection_changed(&self) {}
    fn send_honor_ps1_out_of_sync(&self) {}
}

/// R10 缝: 富内容 secret 检测策略(app ai::blocklist),构造注入。
pub type DetermineSecretLevelFn = fn(&str) -> crate::secrets::SecretLevel;

/// R10: 原 app block_filter 的过滤上下文默认行数,旧路径 re-export 保持单源。
pub const DEFAULT_CONTEXT_LINES_VALUE: u16 = 0;
