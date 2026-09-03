//! R15: BlockList sum_tree 坐标系纯类型(自 app/terminal/model/blocks.rs 提升)。
//! BlockHeightItem(变体载荷含 app view 类型)与 Item impl 留守 app。
use std::ops::AddAssign;
use sum_tree::Dimension;
use warp_terminal::model::BlockIndex;
use warpui::units::{IntoLines, Lines};

/// R15: 自 blocks.rs 提升(纯 Lines newtype)。
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct BlockHeight(pub Lines);

/// Index/dimension of _all_ the items in the BlockHeights sum tree. This includes blocks, banners,
/// gaps, and anything that could be within the BlockList but isn't necessarily a block.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct TotalIndex(pub usize);

impl std::ops::Add<usize> for TotalIndex {
    type Output = TotalIndex;

    fn add(self, rhs: usize) -> Self::Output {
        Self(self.0 + rhs)
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct BlockHeightSummary {
    pub total_count: usize,
    pub height: Lines,
    pub block_count: usize,
}

impl BlockHeight {
    pub const fn zero() -> Self {
        Self(Lines::zero())
    }

    pub fn as_f64(self) -> f64 {
        self.0.as_f64()
    }

    pub fn into_lines(self) -> Lines {
        self.0
    }
}

// R15: BlockListPoint 纯几何部分(from_within_block_point 依赖 BlockList,
// 留守 app 侧 impl BlockListPointExt)。
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct BlockListPoint {
    // TODO(vorporeal): Use `Lines` as the type here.
    pub row: Lines,
    pub column: usize,
}

impl BlockListPoint {
    pub fn new(row: impl IntoLines, column: usize) -> Self {
        BlockListPoint {
            row: row.into_lines(),
            column,
        }
    }
}

impl float_cmp::ApproxEq for BlockListPoint {
    type Margin = float_cmp::F64Margin;

    fn approx_eq<M: Into<Self::Margin>>(self, other: Self, margin: M) -> bool {
        self.row
            .into_lines()
            .approx_eq(other.row.into_lines(), margin)
            && self.column == other.column
    }
}

impl AddAssign<&BlockHeightSummary> for BlockHeightSummary {
    fn add_assign(&mut self, other: &Self) {
        self.height += other.height;
        self.total_count += other.total_count;
        self.block_count += other.block_count;
    }
}

impl From<usize> for TotalIndex {
    fn from(count: usize) -> Self {
        Self(count)
    }
}

impl<T> From<T> for BlockHeight
where
    T: IntoLines,
{
    fn from(value: T) -> Self {
        Self(value.into_lines())
    }
}

impl From<BlockHeight> for f64 {
    fn from(block_height: BlockHeight) -> f64 {
        block_height.0.as_f64()
    }
}

impl From<BlockHeight> for Lines {
    fn from(block_height: BlockHeight) -> Self {
        block_height.0
    }
}

impl<'a> Dimension<'a, BlockHeightSummary> for BlockHeight {
    fn add_summary(&mut self, summary: &'a BlockHeightSummary) {
        self.0 += summary.height
    }
}

impl<'a> Dimension<'a, BlockHeightSummary> for BlockIndex {
    fn add_summary(&mut self, summary: &'a BlockHeightSummary) {
        self.0 += summary.block_count
    }
}

impl<'a> Dimension<'a, BlockHeightSummary> for TotalIndex {
    fn add_summary(&mut self, summary: &'a BlockHeightSummary) {
        self.0 += summary.total_count
    }
}

impl<'a> Dimension<'a, BlockHeightSummary> for BlockHeightSummary {
    fn add_summary(&mut self, summary: &'a BlockHeightSummary) {
        *self += summary
    }
}
