pub use block_list_element::GridType;
use model::alt_screen::AltScreen;
use model::blocks::BlockList;
use model::index::Side;
pub use model::terminal_model::TerminalModel;
use ordered_float::Float;
use pathfinder_geometry::vector::vec2f;
use serde::{Deserialize, Serialize};
use std::cmp::max;
mod package_installers;
pub(crate) use history::UpArrowHistoryConfig;
pub use view::Event;
pub use view::TerminalView;
pub use warp_terminal::shell::{self, ShellLaunchData};
use warpui::geometry::vector::Vector2F;
use warpui::units::{IntoPixels, Lines, Pixels};
use warpui::AppContext;
use warpui::WindowId;
pub use {history::History, history::HistoryEntry, history::HistoryEvent, history::ShellHost};

// R10: SizeInfo/ClipboardType 下沉 terminal_grid_model,旧路径保持兼容。
pub use terminal_grid_model::{ClipboardType, SizeInfo, SizeUpdate, SizeUpdateReason};
mod block_list_settings;

mod alias;
mod alt_screen;
pub mod alt_screen_reporting;
mod audible_bell;
pub use audible_bell::AudibleBell;
pub mod available_shells;

mod block_filter;
pub mod block_list_element;
pub mod block_list_viewport;
pub mod blockgrid_element;
mod blockgrid_renderer;
mod bootstrap;
pub mod color;
mod command_corrections_denylist;
pub mod dynamic_enum_suggestions;
pub mod event;
pub mod event_listener;
pub mod terminal_activity;
pub mod find;
pub mod general_settings;
pub mod grid_renderer;
pub mod grid_size_util;
pub mod history;
pub mod input;
#[cfg(not(target_family = "wasm"))]
pub mod intercept_sessions;
pub mod keys;
pub mod keys_settings;
pub mod ligature_settings;
mod line_editor_status;
pub mod links;
#[cfg(all(not(target_family = "wasm"), feature = "local_tty"))]
pub mod local_shell;
#[cfg(feature = "local_tty")]
pub mod local_tty;
mod meta_shortcuts;
pub mod mock_terminal_manager;
pub mod model;
pub mod model_events;
pub mod platform;
pub mod profile_model_selector;
pub mod prompt;
pub mod prompt_render_helper;
pub mod recorder;
pub mod remote_tty;
pub mod resizable_data;
pub mod rich_history;
pub mod safe_mode_settings;
mod secret_regex_updater;
pub mod session_settings;
pub mod settings;
// Dais:删除 share_block_modal(云端 block 永久链分享)
pub mod shared_session;
mod shell_launch_state;
pub mod universal_developer_input;

pub mod ssh;
pub mod terminal_manager;
mod terminal_size_element;
pub mod view;
pub mod warpify;
mod waterfall_gap_element;
mod writeable_pty;
#[cfg(windows)]
pub mod wsl;

pub mod cli_agent;
pub use cli_agent::CLIAgent;
pub(crate) mod cli_agent_sessions;

pub use mock_terminal_manager::MockTerminalManager;
use model_events::{ModelEvent, ModelEventDispatcher};
// Dais:删除 ShareBlockModal pub use
pub use terminal_manager::TerminalManager;

pub use block_list_settings::*;
pub use secret_regex_updater::CustomSecretRegexUpdater;
pub use view::{
    CANCEL_COMMAND_KEYBINDING, TOGGLE_AUTOEXECUTE_MODE_KEYBINDING,
    TOGGLE_HIDE_CLI_RESPONSES_KEYBINDING, TOGGLE_QUEUE_NEXT_PROMPT_KEYBINDING,
};

pub use shell_launch_state::ShellLaunchState;

/// Minimum number of visible lines.
const MIN_ROWS: usize = 1;

/// Minimum number of columns.
///
/// A minimum of 2 is necessary to hold fullwidth unicode characters.
const MIN_COLUMNS: usize = 2;

/// The broadcast channel capacity for PTY reads.
/// This constant was picked arbitrarily. We really shouldn't
/// fall more than this many reads behind the PTY itself anyways.
/// We also don't want to make this too large because we
/// have to pay the cost of pre-allocating memory for the channel
/// (and the larger this is, the more memory we eagerly allocate).
/// TODO: investigate if we can reduce the number of PTY reads we need to buffer
/// per event loop run.
pub const PTY_READS_BROADCAST_CHANNEL_SIZE: usize = 1024;

pub fn init(app: &mut AppContext) {
    // Dais:删除 share_block_modal::init
    view::init(app);
}

/// Treat rounding errors for heights within this amount as equal.
pub const HEIGHT_FUDGE_FACTOR_LINES: Lines = Lines::new(0.01);

/// Returns whether two heights in lines are approximately equal.
/// This is an annoying cludge to handle the fact that we're using floating point
/// throughout our block heights code and have to deal with the consequences of accumulated
/// rounding errors.
pub fn heights_approx_eq(a: Lines, b: Lines) -> bool {
    (a - b).abs() < HEIGHT_FUDGE_FACTOR_LINES
}

/// Returns whether height a is greater than or equal to height b, allowing
/// for a bit of fudging to account for accumulated rounding errors.
pub fn heights_approx_gte(a: Lines, b: Lines) -> bool {
    a > b || heights_approx_eq(a, b)
}

/// Returns whether height a is greater than height b, allowing for a bit of fudging to account
/// for accumulated rounding errors.
pub fn heights_approx_gt(a: Lines, b: Lines) -> bool {
    a > b && !heights_approx_eq(a, b)
}

/// Returns whether height a is less than or equal to height b, allowing
/// for a bit of fudging to account for accumulated rounding errors.
pub fn heights_approx_lte(a: Lines, b: Lines) -> bool
where
{
    a < b || heights_approx_eq(a, b)
}

/// Returns whether height a is less than height b, allowing for a bit of fudging to account
/// for accumulated rounding errors.
pub fn heights_approx_lt(a: Lines, b: Lines) -> bool {
    a < b && !heights_approx_eq(a, b)
}

/// Returns whether the given height is between the start and end heights,
/// allowing for a bit of fudging to account for accumulated rounding errors.
pub fn height_in_range_approx(height: Lines, start: Lines, end: Lines) -> bool {
    heights_approx_gte(height, start) && heights_approx_lte(height, end)
}

/// Returns the size of the `SavePosition`-ed element with the given ID from the last layout cycle.
///
/// If this is the first app layout, if if there was no laid-out element with the given ID, returns
/// `None`.
pub(crate) fn element_size_at_last_frame(
    element_position_id: &str,
    window_id: WindowId,
    app: &AppContext,
) -> Option<Vector2F> {
    app.element_position_by_id_at_last_frame(window_id, element_position_id)
        .map(|position| position.size())
}


/// Encapsulates info for updating the size of the terminal.


/// Helper struct containing the cell size and window padding info.
pub struct CellSizeAndWindowPadding {
    /// Width of an individual cell.
    pub cell_width_px: Pixels,

    /// Height of an individual cell.
    pub cell_height_px: Pixels,

    /// Horizontal window padding.
    pub padding_x_px: Pixels,

    /// Vertical window padding.
    pub padding_y_px: Pixels,
}


/// The padding around each block, represented in fractional lines.
///
/// TODO(vorporeal): Change this to hold `Lines` instead of `f32`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockPadding {
    /// Padding from top of the block to the prompt.
    pub padding_top: f32,
    /// Padding from bottom of prompt to top of the command.
    pub command_padding_top: f32,
    /// Padding from bottom of command to top of output.
    pub middle: f32,
    /// Padding from bottom of output to bottom of block.
    pub bottom: f32,
}

impl BlockPadding {
    pub fn new(
        padding_top: f32,
        command_padding_top: f32,
        padding_middle: f32,
        padding_bottom: f32,
    ) -> Self {
        BlockPadding {
            padding_top,
            command_padding_top,
            middle: padding_middle,
            bottom: padding_bottom,
        }
    }
}

#[cfg(test)]
mod ref_tests;
