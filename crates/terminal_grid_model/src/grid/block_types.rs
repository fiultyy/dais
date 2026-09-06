//! R14: block 坐标/尺寸纯类型批(自 app block.rs/terminal_model.rs/
//! block_list_element.rs/terminal/mod.rs 下沉)。
use std::ops::RangeInclusive;
use enum_iterator::Sequence;
use crate::block_index::BlockIndex;
use terminal_model::index::Point;

use crate::SizeInfo;

/// R14: 自 app/terminal/mod.rs 下沉(纯 f32×4)。
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

#[derive(Debug, PartialEq, Copy, Clone, Eq, PartialOrd, Sequence)]
pub enum GridType {
    Prompt,
    Rprompt,          // Right side prompt
    PromptAndCommand, // Combined prompt/command grid.
    Output,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd)]
pub struct WithinBlock<T> {
    pub block_index: BlockIndex,
    pub grid: GridType,
    pub inner: T,
}

impl<T> WithinBlock<T> {
    pub fn new(inner: T, block_index: BlockIndex, grid: GridType) -> WithinBlock<T> {
        Self {
            inner,
            block_index,
            grid,
        }
    }

    pub fn is_in_command_content(&self) -> bool {
        self.grid == GridType::PromptAndCommand
    }

    pub fn is_output_grid(&self) -> bool {
        self.grid == GridType::Output
    }

    pub fn in_same_block_and_grid<U>(&self, other: &WithinBlock<U>) -> bool {
        self.block_index == other.block_index && self.grid == other.grid
    }

    pub fn get(&self) -> &T {
        &self.inner
    }

    pub fn replace(&mut self, value: T) {
        self.inner = value;
    }

    pub fn map<F, U>(self, f: F) -> WithinBlock<U>
    where
        F: FnOnce(T) -> U,
    {
        let new_val: U = f(self.inner);
        WithinBlock {
            inner: new_val,
            block_index: self.block_index,
            grid: self.grid,
        }
    }

    pub fn is_within_this_block(&self, index: BlockIndex) -> bool {
        self.block_index == index
    }
}

impl<T: Copy> WithinBlock<RangeInclusive<T>> {
    /// When you wrap a Range in a WithinBlock, this method gives you the ends of the range as
    /// separately wrapped values
    pub fn unfold_range(&self) -> (WithinBlock<T>, WithinBlock<T>) {
        let start_val = WithinBlock::new(*self.inner.start(), self.block_index, self.grid);
        let end_val = WithinBlock::new(*self.inner.end(), self.block_index, self.grid);
        (start_val, end_val)
    }
}

impl WithinBlock<Point> {
    // Check if one point is visually before another in the blocklist.
    pub fn is_visually_before(&self, other: &WithinBlock<Point>, inverted_blocklist: bool) -> bool {
        // If two points are within the same block OR if the blocklist is NOT inverted
        // the visual order should be the same as the points' logical order.
        if self.block_index == other.block_index || !inverted_blocklist {
            self < other
        // Else. The visual order is reverse as the points' logical order.
        } else {
            self > other
        }
    }
}

#[derive(Clone)]
pub struct BlockSize {
    pub block_padding: BlockPadding,
    pub size: SizeInfo,
    pub max_block_scroll_limit: usize,
    pub warp_prompt_height_lines: f32,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BlockGridPoint {
    Prompt(Point),
    Rprompt(Point),
    PromptAndCommand(Point),
    Output(Point),
}

impl From<WithinBlock<Point>> for BlockGridPoint {
    fn from(point: WithinBlock<Point>) -> Self {
        match point.grid {
            GridType::Prompt => BlockGridPoint::Prompt(*point.get()),
            GridType::Rprompt => BlockGridPoint::Rprompt(*point.get()),
            GridType::Output => BlockGridPoint::Output(*point.get()),
            GridType::PromptAndCommand => BlockGridPoint::PromptAndCommand(*point.get()),
        }
    }
}

impl From<BlockGridPoint> for GridType {
    fn from(p: BlockGridPoint) -> GridType {
        match p {
            BlockGridPoint::Output(_) => GridType::Output,
            BlockGridPoint::Prompt(_) => GridType::Prompt,
            BlockGridPoint::Rprompt(_) => GridType::Rprompt,
            BlockGridPoint::PromptAndCommand(_) => GridType::PromptAndCommand,
        }
    }
}

impl BlockGridPoint {
    pub fn grid_point(&self) -> Point {
        match self {
            BlockGridPoint::Output(point) => *point,
            BlockGridPoint::Prompt(point) => *point,
            BlockGridPoint::Rprompt(point) => *point,
            BlockGridPoint::PromptAndCommand(point) => *point,
        }
    }

    pub fn to_within_block_point(self, block_index: BlockIndex) -> WithinBlock<Point> {
        WithinBlock::new(self.grid_point(), block_index, self.into())
    }
}

