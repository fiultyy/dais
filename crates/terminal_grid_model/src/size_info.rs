//! R10: 自 app/src/terminal/mod.rs 下沉的终端几何类型。
//! app 侧经 `pub use` 保持旧路径 crate::SizeInfo 全兼容。
use std::cmp::max;

use pathfinder_geometry::vector::{vec2f, Vector2F};
use serde::{Deserialize, Serialize};
use terminal_model::index::Side;
use ordered_float::Float;
use warpui::units::{IntoPixels, Lines, Pixels};

const MIN_ROWS: usize = 1;
const MIN_COLUMNS: usize = 2;

/// Terminal size info.
///
/// Note that this implements Serialize/Deserialize for ref tests.
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct SizeInfo {
    /// The width of the TerminalView pane.
    ///
    /// This is basically the `x` of the the incoming max size constraint on the TerminalView. This
    /// is represented as a raw float rather than a Vector2F to satisfy the
    /// `Serialize`/`Deserialize` trait requirements.
    pub pane_width_px: f32,

    /// The height of the TerminalView pane.
    ///
    /// This is basically the `y` of the the incoming max size constraint on the TerminalView. This
    /// is represented as a raw float rather than a Vector2F to satisfy the
    /// `Serialize`/`Deserialize` trait requirements.
    pub pane_height_px: f32,

    /// This height in rows that the pty and grid model thinks the terminal is.
    ///
    /// Note that *rows* is always determined as a function of pane size, not
    /// the content element size, which is somewhat counterintuitive.  The reason
    /// is that the content element size changes frequenetly as the input size
    /// changes or the input dissapears for long running commands, but many
    /// programs do not handle size changes while they are running very well.  To
    /// get around this we make them think that rows always comes from the pane size.
    pub rows: usize,

    /// This is width in columns that the pty and grid model thinks the terminal is.
    pub columns: usize,

    /// Width of an individual cell.
    pub cell_width_px: Pixels,

    /// Height of an individual cell.
    pub cell_height_px: Pixels,

    /// Horizontal window padding.
    pub padding_x_px: Pixels,

    /// Vertical window padding.
    pub padding_y_px: Pixels,
}


impl SizeInfo {
    pub fn new(
        pane_size_px: Vector2F,
        cell_width_px: Pixels,
        cell_height_px: Pixels,
        padding_x_px: Pixels,
        padding_y_px: Pixels,
    ) -> SizeInfo {
        let rows = (pane_size_px.y() - 2. * padding_y_px.as_f32()) / cell_height_px.as_f32();
        let columns = (pane_size_px.x() - 2. * padding_x_px.as_f32()) / cell_width_px.as_f32();

        SizeInfo {
            pane_width_px: pane_size_px.x(),
            pane_height_px: pane_size_px.y(),
            columns: max(columns as usize, MIN_COLUMNS),
            rows: max(rows as usize, MIN_ROWS),
            cell_width_px,
            cell_height_px,
            padding_x_px: padding_x_px.floor(),
            padding_y_px: padding_y_px.floor(),
        }
    }

    /// Create SizeInfo for a [`TerminalModel`] instance that doesn't have font metrics,
    /// which comes from either a headless Dais instance or tests.
    pub fn new_without_font_metrics(rows: usize, cols: usize) -> Self {
        let width = cols as f32;
        let height = rows as f32;
        SizeInfo::new(
            vec2f(width, height),
            1.0.into_pixels(), /* cell_width */
            1.0.into_pixels(), /* cell_height */
            Pixels::zero(),    /* padding_x */
            Pixels::zero(),    /* padding_y */
        )
    }

    pub fn with_rows_and_columns(mut self, rows: usize, cols: usize) -> SizeInfo {
        self.columns = cols;
        self.rows = rows;
        self
    }

    /// Returns the side of the cell where the mouse is located.
    pub fn get_mouse_side(&self, position: Vector2F) -> Side {
        let x = position.x() as usize;

        let cell_x = x.saturating_sub(self.padding_x_px().as_f32() as usize)
            % self.cell_width_px().as_f32() as usize;
        let half_cell_width = (self.cell_width_px().as_f32() / 2.0) as usize;

        let additional_padding = (self.pane_width_px().as_f32()
            - self.padding_x_px().as_f32() * 2.)
            % self.cell_width_px().as_f32();
        let end_of_grid =
            self.pane_width_px() - self.padding_x_px() - additional_padding.into_pixels();

        if cell_x > half_cell_width
            // Edge case when mouse leaves the window.
            || x as f32 >= end_of_grid.as_f32()
        {
            Side::Right
        } else {
            Side::Left
        }
    }

    #[inline]
    pub fn pane_size_px(&self) -> Vector2F {
        vec2f(self.pane_width_px, self.pane_height_px)
    }

    #[inline]
    pub fn pane_width_px(&self) -> Pixels {
        self.pane_width_px.into_pixels()
    }

    #[inline]
    pub fn pane_height_px(&self) -> Pixels {
        self.pane_height_px.into_pixels()
    }

    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    #[inline]
    pub fn columns(&self) -> usize {
        self.columns
    }

    #[inline]
    pub fn cell_width_px(&self) -> Pixels {
        self.cell_width_px
    }

    #[inline]
    pub fn cell_height_px(&self) -> Pixels {
        self.cell_height_px
    }

    #[inline]
    pub fn padding_x_px(&self) -> Pixels {
        self.padding_x_px
    }

    #[inline]
    pub fn padding_y_px(&self) -> Pixels {
        self.padding_y_px
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ClipboardType {
    Clipboard,
    Selection,
}

// R13: 自 app/terminal/mod.rs 下沉。
/// The reason that the terminal size is being updated
#[derive(Debug, Copy, Clone)]
pub enum SizeUpdateReason {
    /// Updated because of some general refresh (e.g. a font-size change)
    Refresh,

    /// Updated after the temrinal has been laid out, so some of the element
    /// sizes that drive terminal size may have changed.
    AfterLayout,

    /// The shared session sharer's size changed.
    /// This is only applicable for shared session viewers.
    ///
    /// The resultant [`SizeUpdate`] will use the larger of the
    /// sharer's and viewer's size.
    SharerSizeChanged { num_rows: usize, num_cols: usize },

    /// A viewer reported its terminal size to the sharer.
    /// This is only applicable for shared session sharers.
    ///
    /// The resultant [`SizeUpdate`] will use the viewer's reported
    /// size directly (floored at 1 row and 1 column).
    ViewerSizeReported { num_rows: usize, num_cols: usize },
}
#[derive(Debug, Copy, Clone)]
pub struct SizeUpdate {
    /// The reason for the update.
    pub update_reason: SizeUpdateReason,

    /// The last size info.
    pub last_size: SizeInfo,

    /// The new size info.
    pub new_size: SizeInfo,

    /// The new gap height, if there is one.
    pub new_gap_height: Option<Lines>,

    /// The pane-computed rows before any shared session size adjustments.
    pub natural_rows: usize,

    /// The pane-computed columns before any shared session size adjustments.
    pub natural_cols: usize,
}

impl SizeUpdate {
    /// Whether the reason for the update is a refresh.
    pub fn is_refresh(&self) -> bool {
        matches!(self.update_reason, SizeUpdateReason::Refresh)
    }

    /// Returns whether there was any change with this update.
    pub fn anything_changed(&self) -> bool {
        self.pane_size_changed() || self.gap_height_changed() || self.rows_or_columns_changed()
    }

    pub fn rows_or_columns_changed(&self) -> bool {
        self.last_size.columns() != self.new_size.columns()
            || self.last_size.rows() != self.new_size.rows()
    }

    /// Returns whether the pane size changed with this update
    pub fn pane_size_changed(&self) -> bool {
        // It's fine for this to be a near-exact comparison because pane size
        // is not determined by summing floats like content element size is.
        (self.last_size.pane_size_px().x() - self.new_size.pane_size_px().x()).abs() > f32::EPSILON
            || (self.last_size.pane_size_px().y() - self.new_size.pane_size_px().y()).abs()
                > f32::EPSILON
    }

    /// Returns any new gap height to set with this update
    pub fn new_gap_height(&self) -> Option<Lines> {
        self.new_gap_height
    }

    /// Returns whether the gap height changed with this update
    pub fn gap_height_changed(&self) -> bool {
        self.new_gap_height.is_some()
    }

    /// The pane-computed natural rows before shared session adjustments.
    pub fn natural_rows(&self) -> usize {
        self.natural_rows
    }

    /// The pane-computed natural columns before shared session adjustments.
    pub fn natural_cols(&self) -> usize {
        self.natural_cols
    }

    /// Returns true if this resize was caused by a sharer size change.
    pub fn is_sharer_size_change(&self) -> bool {
        matches!(
            self.update_reason,
            SizeUpdateReason::SharerSizeChanged { .. }
        )
    }
}
