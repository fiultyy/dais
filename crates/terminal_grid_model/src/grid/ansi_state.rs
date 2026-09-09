//! R10: 自 ansi_handler.rs 抽出的 GridHandler ANSI 状态体。
use std::ops::Range;
use std::sync::Arc;

use bounded_vec_deque::BoundedVecDeque;
use warp_terminal::model::ansi::CharsetIndex;
use warp_terminal::model::{KeyboardModes, KeyboardModesApplyBehavior};
use warpui::units::Pixels;
use pathfinder_geometry::vector::Vector2F;

use super::{GridHandler, PerformResetGridChecks, TermMode, KEYBOARD_MODE_STACK_MAX_DEPTH};
use crate::grid::GridEventSink;
use crate::secrets::ObfuscateSecrets;
use crate::SizeInfo;
use terminal_model::index::{Point, VisibleRow};
use warp_terminal::model::ansi::control_sequence_parameters::CursorStyle;
use crate::grid::tab_stops::TabStops;

/// State needed for the grid-level implementation of [`ansi::Handler`].
#[derive(Clone)]
pub struct State {
    /// Information about cell dimensions.
    pub cell_width: usize,
    pub cell_height: usize,

    /// Mode flags.
    pub mode: TermMode,

    /// Tabstops.
    pub tabs: TabStops,

    /// Current style of the cursor.
    pub cursor_style: CursorStyle,

    /// Index into `charsets`, pointing to what ASCII is currently being mapped to.
    pub active_charset: CharsetIndex,

    /// Scroll region.
    ///
    /// Range going from top to bottom of the terminal.
    pub scroll_region: Range<VisibleRow>,

    /// Seam: 事件发送缝(R10: 本 crate 不依赖 app 的 ChannelEventListener/Event,
    /// app 装配处以 GridEventSink 实现注入)。
    pub event_sink: std::sync::Arc<dyn GridEventSink>,

    /// Whether the grid is for the alt screen.
    pub is_alt_screen: bool,

    /// Whether or not to obfuscate secrets on copy, respecting the Safe Mode setting.
    pub obfuscate_secrets: ObfuscateSecrets,

    /// Whether this Grid is in a shell context which supports handling the emoji presentation selector
    /// correctly. Notably, Zsh does NOT support this well in bracketed paste mode (which we use for all Dais
    /// commands), which can lead to cursor misalignment issues.
    pub supports_emoji_presentation_selector: bool,

    /// State related to the Reset Grid logic for ConPTY.
    pub reset_grid_checks: ResetGridChecks,

    /// Range of cells that were dirtied during the current run of byte parsing. See
    /// [`Self::finish_byte_processing`] for where this is reset.
    pub dirty_cells_range: Range<Point>,

    // Dimension of pane.
    pub pane_size: Vector2F,

    /// The currently active keyboard mode.
    pub keyboard_mode: KeyboardModes,

    /// Kitty keyboard enhancement protocol mode stack.
    /// Used purely for push/pop save/restore semantics.
    /// `push` appends the new mode; `pop` truncates and restores
    /// the previous entry as the active mode.
    pub keyboard_mode_stack: BoundedVecDeque<KeyboardModes>,
}

impl State {
    pub fn new(
        size_info: &SizeInfo,
        event_sink: std::sync::Arc<dyn GridEventSink>,
        is_alt_screen: bool,
        obfuscate_secrets: ObfuscateSecrets,
        perform_reset_grid_checks: PerformResetGridChecks,
    ) -> Self {
        let scroll_region = VisibleRow(0)..VisibleRow(size_info.rows());
        let tabs = TabStops::new(size_info.columns());
        let reset_grid_checks = match perform_reset_grid_checks {
            PerformResetGridChecks::Yes => ResetGridChecks::Enabled {
                received_osc: false,
            },
            PerformResetGridChecks::No => ResetGridChecks::Disabled,
        };

        Self {
            cell_width: size_info.cell_width_px.as_f32() as usize,
            cell_height: size_info.cell_height_px.as_f32() as usize,
            mode: Default::default(),
            tabs,
            cursor_style: Default::default(),
            active_charset: Default::default(),
            scroll_region,
            event_sink,
            is_alt_screen,
            obfuscate_secrets,
            // Assume that the Grid supports emoji presentation selector, until set otherwise.
            supports_emoji_presentation_selector: true,
            reset_grid_checks,
            dirty_cells_range: Default::default(),
            pane_size: size_info.pane_size_px(),
            keyboard_mode: KeyboardModes::NO_MODE,
            keyboard_mode_stack: BoundedVecDeque::new(KEYBOARD_MODE_STACK_MAX_DEPTH),
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum ResetGridChecks {
    /// Checks are enabled for the grid.
    Enabled { received_osc: bool },
    /// Checks are disabled for the grid.
    #[default]
    Disabled,
}
