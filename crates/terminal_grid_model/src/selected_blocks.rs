//! R16: SelectedBlocks 选中数学族(自 app/terminal/model/terminal_model.rs 提升)。
//! 纯 BlockIndex 运算;to_block_ids 已参数化为投影闭包。
use std::cmp::{max, min};
use std::ops::RangeInclusive;

use serde::Serialize;
use warp_terminal::model::{BlockId, BlockIndex};

/// Since a SelectedBlockRange is a range of blocks, it is possible that
/// a debug/hidden block is within this range (even though it shouldn't be selected).
/// Care must be taken to ensure that hidden blocks are not considered "selected"
/// when performing actions like "copy".
#[derive(Default, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedBlockRange {
    pivot: BlockIndex,
    tail: BlockIndex,
}

impl SelectedBlockRange {
    pub fn start(&self) -> BlockIndex {
        min(self.pivot, self.tail)
    }

    pub fn end(&self) -> BlockIndex {
        max(self.pivot, self.tail)
    }

    /// Returns the range of the selected index, optionally sorted.
    pub fn range(
        &self,
        sort_direction: Option<BlockSortDirection>,
    ) -> impl Iterator<Item = BlockIndex> {
        let range = self.start().0..=self.end().0;
        // Note we need the heap allocation through the box because we
        // can't return .rev() and not .rev() iterators without it.
        let boxed: Box<dyn Iterator<Item = BlockIndex>> =
            if matches!(sort_direction, Some(BlockSortDirection::MostRecentFirst)) {
                Box::new(range.rev().map(BlockIndex::from))
            } else {
                Box::new(range.map(BlockIndex::from))
            };
        boxed
    }

    fn contains(&self, block_index: BlockIndex) -> bool {
        self.range(None).any(|index| index == block_index)
    }

    fn reversed(&self) -> bool {
        self.pivot > self.tail
    }

    pub fn intersection(
        &self,
        other: &RangeInclusive<BlockIndex>,
    ) -> impl Iterator<Item = BlockIndex> {
        (max(self.start().0, other.start().0)..=min(self.end().0, other.end().0))
            .map(BlockIndex::from)
    }

    pub fn pivot(&self) -> BlockIndex {
        self.pivot
    }
}

#[derive(Copy, Clone, Serialize)]
pub enum BlockSelectionCardinality {
    None,
    One,
    Many,
}

impl BlockSelectionCardinality {
    pub fn as_keymap_context_value(&self) -> &'static str {
        match self {
            BlockSelectionCardinality::None => "None",
            BlockSelectionCardinality::One => "One",
            BlockSelectionCardinality::Many => "Many",
        }
    }
}

/// Wrapper struct around a vector of ranges to provide easier API to use
/// in the context of block selections.
/// Note that although hidden blocks may be within a SelectedBlockRange, the
/// pivot/tail of a SelectedBlockRange will never be a hidden index.
#[derive(Default, Clone, Eq, PartialEq, Debug)]
pub struct SelectedBlocks {
    ranges: Vec<SelectedBlockRange>,
}

/// The direction to sort blocks in.
#[derive(Clone, Copy, Debug)]
pub enum BlockSortDirection {
    // Most recently executed blocks are sorted first.  Matches the inverted
    // block list.
    MostRecentFirst,

    // Most recently executed blocks are sorted last.  Matches the "normal"
    // block list.
    MostRecentLast,
}

impl SelectedBlocks {
    /// Selects all blocks in range [last().pivot, new_tail]
    pub fn range_select(&mut self, new_tail: BlockIndex) {
        if let Some(last_selected_range) = self.ranges.last() {
            // replacing the entire vector ensures we collapse into a single selection range
            self.ranges = vec![SelectedBlockRange {
                pivot: last_selected_range.pivot,
                tail: new_tail,
            }];
        }
    }

    /// Toggles an arbitrary block with index=block_index.
    /// next_index is the first index larger than block_index that is not hidden.
    /// similarly, prior_index is the first index smaller than block_index that is not hidden.
    pub fn toggle(
        &mut self,
        block_index: BlockIndex,
        next_index: Option<BlockIndex>,
        prior_index: Option<BlockIndex>,
    ) {
        let next_index = next_index.unwrap_or(block_index);
        let prior_index = prior_index.unwrap_or(block_index);

        let mut position_of_next_index_in_ranges: Option<usize> = None;
        let mut position_of_prior_index_in_ranges: Option<usize> = None;

        // Determine if block_index is already selected,
        // and if so, which range is it part of
        let mut range_idx: Option<BlockIndex> = None;
        for (idx, block_range) in self.ranges.iter().enumerate() {
            if block_range.contains(block_index) {
                range_idx = Some(idx.into());
            }
            if block_range.contains(next_index) {
                position_of_next_index_in_ranges = Some(idx);
            }
            if block_range.contains(prior_index) {
                position_of_prior_index_in_ranges = Some(idx);
            }
        }

        // If block_index is already selected, then toggling it should deselect it.
        // There are a few cases to consider in the deselection case:
        // 1. The range that block_index belongs to is entirely the block_index.
        //    In this case, we should completely remove the selection range.
        // 2. The block_index is either the pivot or tail of the range it belongs to.
        //    In this case, we should adjust the pivot or tail, respectively,
        //    based on the next_index and prior_index._
        // 3. The block_index is somewhere in the middle of the range it belongs to.
        //    In this case, we should split up the range at the block_index into
        //    two ranges, using the next_index and prior_index accordingly.
        if let Some(idx) = range_idx {
            let idx = idx.0;
            let block_range = self.ranges[idx];

            let (new_pivot, new_tail) = if block_range.reversed() {
                (prior_index, next_index)
            } else {
                (next_index, prior_index)
            };

            // if block_index is the pivot and the tail of the range it is part of
            // then this is case 1: block_range is simply a singleton
            // and should be completely removed since we are toggling it off
            if block_index == block_range.pivot && block_index == block_range.tail {
                self.ranges.remove(idx);
            } else if block_index == block_range.pivot {
                // if block_index is the pivot of the range it is part of (and not also the tail),
                // this is case 2 (pivot): we just need to select a new pivot
                self.ranges[idx].pivot = new_pivot;
            } else if block_index == block_range.tail {
                // if block_index is the tail of the range it is part of (and not also the pivot),
                // this is case 2 (tail): we just need to select a new tail
                self.ranges[idx].tail = new_tail;
            } else {
                // otherwise, this is case 3: if block_index is somewhere in the middle of the range it is part of,
                // then we need to break up this range into two ranges (around block_index).
                // the existing range will end at the new_tail and we'll add a new range
                // right before the existing one with the new_pivot and the tail being the old tail
                self.ranges[idx].tail = new_tail;
                self.ranges.insert(
                    idx,
                    SelectedBlockRange {
                        pivot: new_pivot,
                        tail: block_range.tail,
                    },
                );
            }
        } else {
            // Otherwise, we are in the toggling ON case.

            // We should be able to merge selections as long as at least one of
            // next_index and prior_index is already selected (and not the same selection)
            let can_merge = position_of_next_index_in_ranges != position_of_prior_index_in_ranges;

            // If block_index isn't already selected, then it's a new selection.
            // First, try to merge this new selection with existing selections.
            let new_selection = if can_merge {
                // If this new selection sits directly in between two other
                // selection ranges, merge them all into one selection range.
                if let (
                    Some(position_of_next_index_in_ranges),
                    Some(position_of_prior_index_in_ranges),
                ) = (
                    position_of_next_index_in_ranges,
                    position_of_prior_index_in_ranges,
                ) {
                    // important: remove in reverse order to prevent shifting
                    // the index of the counterpart!
                    let (prior_range_removed, next_range_removed) =
                        if position_of_next_index_in_ranges > position_of_prior_index_in_ranges {
                            let next_range = self.ranges.remove(position_of_next_index_in_ranges);
                            (
                                self.ranges.remove(position_of_prior_index_in_ranges),
                                next_range,
                            )
                        } else {
                            (
                                self.ranges.remove(position_of_prior_index_in_ranges),
                                self.ranges.remove(position_of_next_index_in_ranges),
                            )
                        };
                    Some((prior_range_removed.start(), next_range_removed.end()))
                } else if let Some(position_of_next_index_in_ranges) =
                    position_of_next_index_in_ranges
                {
                    // Otherwise, if this new selection is right before another
                    // selection range, then merge it together.
                    let next_range_removed = self.ranges.remove(position_of_next_index_in_ranges);
                    if next_range_removed.tail == next_index {
                        Some((next_range_removed.pivot, block_index))
                    } else {
                        Some((block_index, next_range_removed.tail))
                    }
                } else if let Some(position_of_prior_index_in_ranges) =
                    position_of_prior_index_in_ranges
                {
                    // Otherwise, this new selection must be right after another
                    // selection range, so merge it together as well.
                    let prior_range_removed = self.ranges.remove(position_of_prior_index_in_ranges);
                    if prior_range_removed.tail == prior_index {
                        Some((prior_range_removed.pivot, block_index))
                    } else {
                        Some((block_index, prior_range_removed.tail))
                    }
                } else {
                    // At least one of next_index, prior_index should be set if
                    // we can merge selections so this should be unreachable
                    log::warn!(
                        "Expected to merge block selections but 'position_of_next/prior_index_in_ranges' were both None."
                    );
                    None
                }
            } else {
                // If we can't merge with an existing selection, then this is disjoint.
                Some((block_index, block_index))
            };
            if let Some((new_pivot, new_tail)) = new_selection {
                self.ranges.push(SelectedBlockRange {
                    pivot: new_pivot,
                    tail: new_tail,
                });
            }
        }
    }

    /// Remove all block selections.
    pub fn reset(&mut self) {
        self.ranges = vec![];
    }

    /// Reset all block selections to unselected except block_index
    pub fn reset_to_single(&mut self, block_index: BlockIndex) {
        let singleton = SelectedBlockRange {
            pivot: block_index,
            tail: block_index,
        };
        self.ranges = vec![singleton];
    }

    /// Reset all block selections to a sequence of single block `SelectedBlockRange`s of the given
    /// `block_indices`. Does not merge the `SelectedBlockRange`s.`
    pub fn reset_to_block_indices(&mut self, block_indices: impl IntoIterator<Item = BlockIndex>) {
        self.ranges = block_indices
            .into_iter()
            .map(|block_index| SelectedBlockRange {
                pivot: block_index,
                tail: block_index,
            })
            .collect::<Vec<_>>();
    }

    /// Returns true iff block_index is selected
    pub fn is_selected(&self, block_index: BlockIndex) -> bool {
        self.ranges
            .iter()
            .any(|block_range| block_range.contains(block_index))
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    // A "singleton" is a single selected block. That is, there is only
    // one range R and R.pivot == R.tail
    pub fn is_singleton(&self) -> bool {
        match &self.ranges[..] {
            [single] => single.pivot == single.tail,
            _ => false,
        }
    }

    /// The `tail` of all selected blocks is defined as the tail of the most
    /// recent selected block range
    pub fn tail(&self) -> Option<BlockIndex> {
        self.ranges
            .last()
            .map(|most_recent_range| most_recent_range.tail)
    }

    pub fn ranges(&self) -> &[SelectedBlockRange] {
        &self.ranges[..]
    }

    /// Returns an iterator whose items are all the selected block indices. Does not guarantee any
    /// order.
    pub fn block_indices(&self) -> impl Iterator<Item = BlockIndex> + '_ {
        self.ranges
            .iter()
            .flat_map(|selected_block_range| selected_block_range.range(None))
    }

    /// Used to produce a consistent list of selected indices to take actions on
    pub fn sorted_ranges(&self, sort_direction: BlockSortDirection) -> Vec<SelectedBlockRange> {
        let mut sorted_ranges = self.ranges.clone();
        sorted_ranges.sort_by_key(|a| a.start());
        if matches!(sort_direction, BlockSortDirection::MostRecentFirst) {
            sorted_ranges.reverse();
        }
        sorted_ranges
    }

    pub fn cardinality(&self) -> BlockSelectionCardinality {
        if self.is_empty() {
            BlockSelectionCardinality::None
        } else if self.is_singleton() {
            BlockSelectionCardinality::One
        } else {
            BlockSelectionCardinality::Many
        }
    }

    /// R16: 参数化投影(&BlockList 依赖留 app 侧调用点闭包)。
    pub fn to_block_ids<'a>(
        &'a self,
        mut block_id_at: impl FnMut(BlockIndex) -> Option<&'a BlockId> + 'a,
    ) -> impl Iterator<Item = &'a BlockId> + 'a {
        self.ranges
            .iter()
            .flat_map(|range| range.range(None))
            .filter_map(move |idx| block_id_at(idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selected_block_range_contains() {
        let range = SelectedBlockRange {
            pivot: 10.into(),
            tail: 2.into(),
        };
        assert!(range.contains(4.into()));
        assert!(!range.contains(1.into()));
    }

    #[test]
    fn test_selected_blocks_is_selected() {
        let selected_blocks = SelectedBlocks {
            ranges: vec![
                SelectedBlockRange {
                    pivot: 3.into(),
                    tail: 4.into(),
                },
                SelectedBlockRange {
                    pivot: 0.into(),
                    tail: 1.into(),
                },
            ],
        };

        assert!(selected_blocks.is_selected(0.into()));
        assert!(!selected_blocks.is_selected(2.into()));
    }

    #[test]
    fn test_selected_blocks_tail() {
        let selected_blocks = SelectedBlocks {
            ranges: vec![
                SelectedBlockRange {
                    pivot: 3.into(),
                    tail: 4.into(),
                },
                SelectedBlockRange {
                    pivot: 0.into(),
                    tail: 1.into(),
                },
            ],
        };

        assert_eq!(selected_blocks.tail(), Some(1.into()));
    }

    #[test]
    fn test_selected_blocks_reset() {
        let mut selected_blocks = SelectedBlocks {
            ranges: vec![
                SelectedBlockRange {
                    pivot: 3.into(),
                    tail: 4.into(),
                },
                SelectedBlockRange {
                    pivot: 0.into(),
                    tail: 1.into(),
                },
            ],
        };

        // Reset to nothing.
        selected_blocks.reset();
        assert!(selected_blocks.is_empty());
        assert!(selected_blocks.tail().is_none());

        // Reset to single.
        selected_blocks.reset_to_single(5.into());
        assert!(!selected_blocks.is_empty());
        assert_eq!(selected_blocks.tail(), Some(5.into()));
    }

    #[test]
    fn test_selected_blocks_range_select() {
        let mut selected_blocks = SelectedBlocks {
            ranges: vec![
                SelectedBlockRange {
                    pivot: 0.into(),
                    tail: 1.into(),
                },
                SelectedBlockRange {
                    pivot: 3.into(),
                    tail: 5.into(),
                },
            ],
        };

        // Range select should reset to a single selection with
        // same pivot as most recent selection, and new tail.
        selected_blocks.range_select(6.into());
        assert_eq!(selected_blocks.ranges().len(), 1);
        assert_eq!(selected_blocks.ranges().last().unwrap().pivot, 3.into());
        assert_eq!(selected_blocks.tail(), Some(6.into()));

        // Range select reversed should work similarly.
        selected_blocks.ranges = vec![
            SelectedBlockRange {
                pivot: 0.into(),
                tail: 1.into(),
            },
            SelectedBlockRange {
                pivot: 3.into(),
                tail: 5.into(),
            },
        ];
        selected_blocks.range_select(0.into());
        assert_eq!(selected_blocks.ranges().len(), 1);
        assert_eq!(selected_blocks.ranges().last().unwrap().pivot, 3.into());
        assert_eq!(selected_blocks.tail(), Some(0.into()));
    }

    #[test]
    fn test_selected_blocks_sorted_ranges() {
        let selected_blocks = SelectedBlocks {
            ranges: vec![
                SelectedBlockRange {
                    pivot: 0.into(),
                    tail: 1.into(),
                },
                SelectedBlockRange {
                    pivot: 3.into(),
                    tail: 5.into(),
                },
            ],
        };

        let sorted_ranges = selected_blocks.sorted_ranges(BlockSortDirection::MostRecentLast);
        assert_eq!(2, sorted_ranges.len());
        assert_eq!(sorted_ranges.first().unwrap().start(), 0.into());
        assert_eq!(sorted_ranges.first().unwrap().end(), 1.into());
        assert_eq!(sorted_ranges.last().unwrap().start(), 3.into());
        assert_eq!(sorted_ranges.last().unwrap().end(), 5.into());

        let reverse_sorted = selected_blocks.sorted_ranges(BlockSortDirection::MostRecentFirst);
        assert_eq!(2, sorted_ranges.len());
        assert_eq!(
            reverse_sorted
                .first()
                .unwrap()
                .range(Some(BlockSortDirection::MostRecentFirst))
                .next()
                .unwrap(),
            5.into()
        );
        assert_eq!(
            reverse_sorted
                .last()
                .unwrap()
                .range(Some(BlockSortDirection::MostRecentFirst))
                .last()
                .unwrap(),
            0.into()
        );
    }

    #[test]
    fn test_selected_blocks_toggle_on() {
        let mut selected_blocks = SelectedBlocks {
            ranges: vec![SelectedBlockRange {
                pivot: 0.into(),
                tail: 3.into(),
            }],
        };

        // Toggle a disjoint selection - should toggle ON
        selected_blocks.toggle(6.into(), Some(7.into()), Some(5.into()));
        assert_eq!(selected_blocks.ranges().len(), 2);
        assert_eq!(selected_blocks.ranges().last().unwrap().pivot, 6.into());
        assert_eq!(selected_blocks.ranges().last().unwrap().tail, 6.into());
        assert!(selected_blocks.is_selected(6.into()));

        // Toggle a selection before another, should merge
        selected_blocks.toggle(5.into(), Some(6.into()), Some(4.into()));
        assert_eq!(selected_blocks.ranges().len(), 2);
        assert_eq!(selected_blocks.ranges().last().unwrap().pivot, 6.into());
        assert_eq!(selected_blocks.ranges().last().unwrap().tail, 5.into());
        assert!(selected_blocks.is_selected(5.into()));

        // Toggle a selection at the end of another, should merge
        selected_blocks.toggle(7.into(), Some(8.into()), Some(6.into()));
        assert_eq!(selected_blocks.ranges().len(), 2);
        assert_eq!(selected_blocks.ranges().last().unwrap().pivot, 7.into());
        assert_eq!(selected_blocks.ranges().last().unwrap().tail, 5.into());
        assert!(selected_blocks.is_selected(7.into()));

        // Toggle a selection between two existing selections, should merge
        selected_blocks.toggle(4.into(), Some(5.into()), Some(3.into()));
        assert_eq!(selected_blocks.ranges().len(), 1);
        assert_eq!(selected_blocks.ranges().last().unwrap().pivot, 0.into());
        assert_eq!(selected_blocks.ranges().last().unwrap().tail, 7.into());
        assert!(selected_blocks.is_selected(4.into()));
    }

    #[test]
    fn test_selected_blocks_toggle_off() {
        let mut selected_blocks = SelectedBlocks {
            ranges: vec![
                SelectedBlockRange {
                    pivot: 2.into(),
                    tail: 2.into(),
                },
                SelectedBlockRange {
                    pivot: 0.into(),
                    tail: 1.into(),
                },
                SelectedBlockRange {
                    pivot: 25.into(),
                    tail: 20.into(),
                },
                SelectedBlockRange {
                    pivot: 3.into(),
                    tail: 10.into(),
                },
            ],
        };

        // Case 1: deselect the entire selection range
        selected_blocks.toggle(2.into(), Some(3.into()), Some(1.into()));
        assert_eq!(selected_blocks.ranges().len(), 3);
        assert!(!selected_blocks.is_selected(2.into()));

        // Case 2: deselect the pivot and tail for non-reversed range
        selected_blocks.toggle(25.into(), Some(26.into()), Some(24.into())); // deselect pivot
        selected_blocks.toggle(20.into(), Some(21.into()), Some(19.into())); // deselect tail
        assert_eq!(selected_blocks.ranges().len(), 3);
        assert_eq!(selected_blocks.ranges()[1].pivot, 24.into());
        assert_eq!(selected_blocks.ranges()[1].tail, 21.into());
        assert!(!selected_blocks.is_selected(25.into()));
        assert!(!selected_blocks.is_selected(20.into()));

        // Case 2: deselect the pivot and tail for reversed range
        selected_blocks.toggle(3.into(), Some(4.into()), Some(2.into())); // deselect pivot
        selected_blocks.toggle(10.into(), Some(11.into()), Some(9.into())); // deselect tail
        assert_eq!(selected_blocks.ranges().len(), 3);
        assert_eq!(selected_blocks.ranges()[2].pivot, 4.into());
        assert_eq!(selected_blocks.ranges()[2].tail, 9.into());
        assert!(!selected_blocks.is_selected(3.into()));
        assert!(!selected_blocks.is_selected(10.into()));

        // Case 3: deselect in the middle of range
        selected_blocks.toggle(6.into(), Some(7.into()), Some(5.into()));
        assert_eq!(selected_blocks.ranges().len(), 4);
        assert_eq!(selected_blocks.ranges()[2].pivot, 7.into());
        assert_eq!(selected_blocks.ranges()[2].tail, 9.into());
        assert_eq!(selected_blocks.ranges()[3].pivot, 4.into());
        assert_eq!(selected_blocks.ranges()[3].tail, 5.into());
        assert!(!selected_blocks.is_selected(6.into()));
    }
}
