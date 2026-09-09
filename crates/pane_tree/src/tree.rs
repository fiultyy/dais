//! 树结构纯算法层 (pane_tree v2, 自 app/src/pane_group/tree.rs 下沉:
//! PaneData/PaneNode/PaneBranch/HiddenPane/Divider/Direction/DraggedBorder
//! 纯类型与纯算法 impl)。
//!
//! 硬边界: 宿主耦合方法 (render/render_pane_id/element_position/ViewContext<
//! PaneGroup>) 留守 app/src/pane_group/tree.rs, 以 extension trait
//! (`PaneDataHost` 等) 挂回这些类型; 本模块对 app 业务类型零感知。
//!
//! 可见性: 原 `pub(in crate::pane_group)` 约束随 crate 边界放宽为 `pub`;
//! `PaneBranch.axis/dividers`、`Divider.id/mouse_state` 放宽为 pub —— app 侧
//! divider 渲染/resize glue 需直接读取。

use std::collections::HashSet;
use std::{fmt, iter, mem};
use crate::PaneId;
use pathfinder_geometry::rect::RectF;
use warpui::elements::MouseStateHandle;
use warpui::EntityId;

pub use crate::snapshot::{PaneFlex, SplitDirection};

pub const DEFAULT_FLEX_VALUE: f32 = 1.0;
pub const DEFAULT_FLEX_SIZE: PaneFlex = PaneFlex(DEFAULT_FLEX_VALUE);

/// Tree for all of the split panes
///
/// Holds the root node and maintains the size of the tree
///
/// Also has an option hidden pane id, if you ever want a pane
/// to remain in the tree but not be rendered, which is needed
/// for pane drag and dropping
pub struct PaneData {
    pub root: PaneNode,
    len: usize,
    hidden_panes: Vec<HiddenPane>,
}

#[derive(Debug, Clone, Copy)]
pub struct HiddenPane {
    pub pane_id: PaneId,
    reason: HiddenPaneReason,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HiddenPaneReason {
    FromMove,
    FromJob,
    TemporaryReplacement(PaneId),

    // Pane was closed. We keep it around temporarily in case
    // the user wants to undo the close.
    Closed,

    // Pane is a child agent spawned by an orchestrator. It stays hidden
    // until the user explicitly reveals it from the status card.
    ChildAgent,
}

impl HiddenPane {
    pub fn from_move(pane_id: PaneId) -> Self {
        Self {
            pane_id,
            reason: HiddenPaneReason::FromMove,
        }
    }
    pub fn from_job(pane_id: PaneId) -> Self {
        Self {
            pane_id,
            reason: HiddenPaneReason::FromJob,
        }
    }
    pub fn from_temporary_replacement(pane_id: PaneId, replacement_pane_id: PaneId) -> Self {
        Self {
            pane_id,
            reason: HiddenPaneReason::TemporaryReplacement(replacement_pane_id),
        }
    }
    pub fn from_close(pane_id: PaneId) -> Self {
        Self {
            pane_id,
            reason: HiddenPaneReason::Closed,
        }
    }
    pub fn from_child_agent(pane_id: PaneId) -> Self {
        Self {
            pane_id,
            reason: HiddenPaneReason::ChildAgent,
        }
    }
}

/// Single Node in the tree of panes
pub enum PaneNode {
    /// A collection of panes split in a specific direction
    Branch(PaneBranch),
    /// A single pane
    Leaf(PaneId),
}

pub struct PaneBranch {
    pub axis: SplitDirection,
    pub nodes: Vec<(PaneFlex, PaneNode)>,
    pub dividers: Vec<Divider>,
}

/// The result of attempting to remove a pane from a branch
enum BranchRemoveResult {
    /// The pane was not found in this sub-tree
    NotFound,
    /// The pane was found and removed, no further action is needed
    Removed,
    /// The pane was found and removed, leaving only a single node in the branch, so it needs to
    /// be collapsed into the parent
    Collapse(PaneNode),
}

/// The result of attempting to find a pane with direction
#[derive(Debug, PartialEq)]
pub enum FindPaneByDirectionResult {
    /// Located the current pane in the tree.
    Located,
    /// The current pane is not found in the tree.
    NotFound,
    /// A list of possible target panes were found.
    Found(HashSet<PaneId>),
}

pub trait FindPaneByDirection {
    fn panes_by_direction(
        &self,
        content: PaneId,
        direction: Direction,
    ) -> FindPaneByDirectionResult;
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn axis(&self) -> SplitDirection {
        match self {
            Direction::Left | Direction::Right => SplitDirection::Horizontal,
            Direction::Up | Direction::Down => SplitDirection::Vertical,
        }
    }
}

pub struct Divider {
    pub id: EntityId,
    pub mouse_state: MouseStateHandle,
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}

impl Divider {
    pub fn new() -> Self {
        Self {
            id: EntityId::new(),
            mouse_state: Default::default(),
        }
    }
}

impl PaneData {
    /// Hidden pane 表视图 (app 侧 divider 渲染 glue 消费; 表本身由纯方法维护)。
    pub fn hidden_panes(&self) -> &[HiddenPane] {
        &self.hidden_panes
    }

    /// Create a new `PaneData` with a Leaf as the root
    pub fn new(pane_id: PaneId) -> Self {
        Self {
            root: PaneNode::Leaf(pane_id),
            len: 1,
            hidden_panes: Vec::new(),
        }
    }

    pub fn visible_pane_count(&self) -> usize {
        let total_panes = self.pane_ids().len();
        let hidden_count = self.num_hidden_panes();
        total_panes.saturating_sub(hidden_count)
    }

    pub fn has_horizontal_split(&self) -> bool {
        self.root.has_horizontal_split(&self.hidden_panes)
    }

    pub fn num_hidden_panes(&self) -> usize {
        self.hidden_panes.len()
    }

    pub fn remove_hidden_pane(&mut self, pane_id: PaneId) {
        self.hidden_panes.retain(|pane| pane.pane_id != pane_id);
    }

    /// Create a new `PaneData` with a Branch as the root
    ///
    /// Note: If there is only a single top-level Node (e.g. from a manually edited launch
    /// configuration), then this will collapse that Node into the root of this `PaneData`
    pub fn new_branch(axis: SplitDirection, nodes: Vec<(PaneFlex, PaneNode)>, len: usize) -> Self {
        let root = if nodes.len() == 1 {
            let mut mutable_nodes = nodes;
            // Safety: We know there is exactly one node in the list
            mutable_nodes.pop().unwrap().1
        } else {
            let dividers = iter::repeat_with(Divider::new)
                .take(nodes.len() - 1)
                .collect();
            PaneNode::Branch(PaneBranch {
                axis,
                nodes,
                dividers,
            })
        };

        Self {
            root,
            len,
            hidden_panes: Vec::new(),
        }
    }

    pub fn move_pane(&mut self, id: PaneId, target_pane_id: PaneId, direction: Direction) -> bool {
        if id == target_pane_id {
            return false;
        }

        // If the given move would not result in the pane tree being mutated, just return early
        if self.sibling_by_direction(target_pane_id, direction) == Some(id) {
            return false;
        }

        // Remove the pane from the tree
        if !self.remove(id) {
            log::error!("Pane not found");
            return false;
        }

        // Call a new split to move the pane to the new location
        self.split(target_pane_id, id, direction)
    }

    pub fn hide_pane_for_move(&mut self, id: PaneId) {
        self.hidden_panes.push(HiddenPane::from_move(id));
    }

    pub fn clear_hidden_panes_from_move(&mut self) {
        self.hidden_panes
            .retain(|pane| pane.reason != HiddenPaneReason::FromMove);
    }

    pub fn hide_pane_for_job(&mut self, id: PaneId) {
        self.hidden_panes.push(HiddenPane::from_job(id));
    }

    pub fn show_pane_for_job(&mut self, id: PaneId) {
        if let Some(pos) = self
            .hidden_panes
            .iter()
            .position(|pane| pane.pane_id == id && pane.reason == HiddenPaneReason::FromJob)
        {
            self.hidden_panes.remove(pos);
        } else {
            log::error!("Attempted to show pane for the job but couldn't find it.")
        }
    }

    pub fn hide_pane_for_child_agent(&mut self, id: PaneId) {
        if !self.is_pane_hidden(&id) {
            self.hidden_panes.push(HiddenPane::from_child_agent(id));
        }
    }

    pub fn show_pane_for_child_agent(&mut self, id: PaneId) {
        if let Some(pos) = self
            .hidden_panes
            .iter()
            .position(|pane| pane.pane_id == id && pane.reason == HiddenPaneReason::ChildAgent)
        {
            self.hidden_panes.remove(pos);
        } else {
            log::error!("Attempted to show child agent pane but couldn't find it.")
        }
    }

    pub fn toggle_pane_visibility_for_job(&mut self, id: PaneId) -> bool {
        if pane_hidden_for_job(&self.hidden_panes, &id) {
            self.show_pane_for_job(id);
            true
        } else {
            self.hide_pane_for_job(id);
            false
        }
    }

    pub fn hide_closed_pane(&mut self, id: PaneId) {
        self.hidden_panes.push(HiddenPane::from_close(id));
    }

    pub fn unhide_closed_pane(&mut self, id: PaneId) -> bool {
        if let Some(pos) = self.hidden_panes.iter().position(|pane| pane.pane_id == id) {
            self.hidden_panes.remove(pos);
            true
        } else {
            log::warn!(
                "Attempted to show pane {id} for undo close but couldn't find it in hidden panes"
            );
            false
        }
    }

    pub fn get_closed_pane_ids(&self) -> Vec<PaneId> {
        self.hidden_panes
            .iter()
            .filter(|hidden| matches!(hidden.reason, HiddenPaneReason::Closed))
            .map(|hidden| hidden.pane_id)
            .collect()
    }

    pub fn clear_hidden_closed_panes(&mut self) {
        self.hidden_panes
            .retain(|pane| pane.reason != HiddenPaneReason::Closed);
    }

    pub fn is_temporary_replacement(&self, replacement_pane_id: PaneId) -> bool {
        self.original_pane_for_replacement(replacement_pane_id)
            .is_some()
    }

    pub fn original_pane_for_replacement(&self, replacement_pane_id: PaneId) -> Option<PaneId> {
        self.hidden_panes.iter().find_map(|hidden_pane| {
            matches!(hidden_pane.reason, HiddenPaneReason::TemporaryReplacement(id) if id == replacement_pane_id)
                .then_some(hidden_pane.pane_id)
        })
    }

    pub fn is_hidden_closed_pane(&self, pane_id: &PaneId) -> bool {
        self.hidden_panes
            .iter()
            .any(|hidden_pane| hidden_pane.pane_id == *pane_id)
    }

    pub fn replace_pane(
        &mut self,
        original_pane_id: PaneId,
        replacement_pane_id: PaneId,
        is_temporary: bool,
    ) -> bool {
        // First, check if the original pane exists in the tree
        if !self.root.contains_pane(original_pane_id) {
            return false;
        }

        // Hide the original pane for temporary replacement
        if is_temporary {
            self.hidden_panes
                .push(HiddenPane::from_temporary_replacement(
                    original_pane_id,
                    replacement_pane_id,
                ));
        }

        // Replace the original pane with the replacement pane in the tree
        let success = self
            .root
            .replace_pane(original_pane_id, replacement_pane_id);

        if success {
            return true;
        } else if is_temporary {
            // If our pane replacement failed, remove the newly added pane from the hidden panes list
            self.hidden_panes.pop();
        }
        false
    }

    pub fn revert_temporary_replacement(&mut self, replacement_pane_id: PaneId) -> Option<PaneId> {
        // Find and remove the hidden pane that corresponds to this replacement
        if let Some(position) = self.hidden_panes.iter().position(|hidden_pane| {
            matches!(hidden_pane.reason, HiddenPaneReason::TemporaryReplacement(id) if id == replacement_pane_id)
        }) {
            let hidden_pane = self.hidden_panes.remove(position);
            let original_pane_id = hidden_pane.pane_id;

            // Replace the replacement pane with the original pane in the tree
            if self.root.replace_pane(replacement_pane_id, original_pane_id) {
                Some(original_pane_id)
            } else {
                // If replacement failed, re-add the hidden pane entry
                self.hidden_panes.insert(position, hidden_pane);
                None
            }
        } else {
            None
        }
    }

    pub fn split(&mut self, old_id: PaneId, new_id: PaneId, direction: Direction) -> bool {
        let successful_split = self.root.split(old_id, new_id, direction);

        if successful_split {
            self.len += 1;
        }

        successful_split
    }

    /// Split the root of the pane tree, inserting `new_id` according to the given direction.
    pub fn split_root(&mut self, new_id: PaneId, direction: Direction) {
        self.root.insert(new_id, direction);
        self.len += 1;
    }

    pub fn remove(&mut self, content: PaneId) -> bool {
        let successful_remove = self.root.remove(content);

        if successful_remove {
            self.len = self.len.saturating_sub(1);
        }

        successful_remove
    }

    /// Get the child panes in an array sorted from left to right, up to down.
    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.root.pane_ids()
    }

    /// Get only the visible child panes in an array sorted from left to right, up to down.
    /// This filters out panes that are hidden for any reason (move, job, close, etc.).
    pub fn visible_pane_ids(&self) -> Vec<PaneId> {
        self.root
            .pane_ids()
            .into_iter()
            .filter(|pane_id| !self.is_pane_hidden(pane_id))
            .collect()
    }

    /// Returns true if the given pane is hidden for any reason.
    pub fn is_pane_hidden(&self, pane_id: &PaneId) -> bool {
        self.hidden_panes
            .iter()
            .any(|hidden_pane| hidden_pane.pane_id == *pane_id)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn are_rects_overlapping(rect1: &RectF, rect2: &RectF, axis: SplitDirection) -> bool {
        // Returns true if the two rectangles overlap in the given axis.
        //
        //                 ---------
        //  -----------    | rect2 |
        // | rect1     |   ---------
        // |           |
        //  -----------
        //
        // In this case, the function would return true for SplitDirection::Horizontal.
        // It would return false for SplitDirection::Vertical.
        match axis {
            SplitDirection::Horizontal => {
                !(rect1.max_y() <= rect2.min_y() || rect1.min_y() >= rect2.max_y())
            }
            SplitDirection::Vertical => {
                !(rect1.max_x() <= rect2.min_x() || rect1.min_x() >= rect2.max_x())
            }
        }
    }

    // Find a pane from the given pane in the given direction, but only if it is a direct sibling
    //  of the given pane.  This means they are direct children of the same branch.
    pub fn sibling_by_direction(&self, pane_id: PaneId, direction: Direction) -> Option<PaneId> {
        match &self.root {
            PaneNode::Branch(b) => b.sibling_by_direction(pane_id, direction),
            _ => None,
        }
    }
}

impl PaneNode {
    pub fn has_visible_children(&self, hidden_panes: &[HiddenPane]) -> bool {
        match self {
            PaneNode::Leaf(pane_id) => {
                !pane_hidden_for_job(hidden_panes, pane_id)
                    && !pane_hidden_for_undo(hidden_panes, pane_id)
                    && !pane_hidden_for_move(hidden_panes, pane_id)
                    && !pane_hidden_for_child_agent(hidden_panes, pane_id)
            }
            PaneNode::Branch(branch) => branch.has_visible_children(hidden_panes),
        }
    }

    pub fn has_children_hidden_for_move(&self, hidden_panes: &[HiddenPane]) -> bool {
        match self {
            PaneNode::Leaf(pane_id) => pane_hidden_for_move(hidden_panes, pane_id),
            PaneNode::Branch(branch) => branch.has_children_hidden_for_move(hidden_panes),
        }
    }

    pub fn has_horizontal_split(&self, hidden_panes: &[HiddenPane]) -> bool {
        match self {
            PaneNode::Leaf(_) => false,
            PaneNode::Branch(branch) => {
                let mut visible_or_move_children = 0usize;
                let mut any_child_split = false;

                for (_, child) in &branch.nodes {
                    if !child.has_visible_children(hidden_panes)
                        && !child.has_children_hidden_for_move(hidden_panes)
                    {
                        continue;
                    }

                    visible_or_move_children += 1;

                    if child.has_horizontal_split(hidden_panes) {
                        any_child_split = true;
                    }
                }

                let self_has_split =
                    branch.axis == SplitDirection::Horizontal && visible_or_move_children > 1;

                self_has_split || any_child_split
            }
        }
    }

    fn split(&mut self, old_pane_id: PaneId, new_pane_id: PaneId, direction: Direction) -> bool {
        match self {
            PaneNode::Leaf(pane) => {
                if *pane == old_pane_id {
                    *self = PaneNode::Branch(PaneBranch::for_leaves(
                        old_pane_id,
                        new_pane_id,
                        direction,
                    ));
                    true
                } else {
                    false
                }
            }
            PaneNode::Branch(branch) => branch.split(old_pane_id, new_pane_id, direction),
        }
    }

    /// Number of splits at the node in the given axis. For leaf nodes, this is always one.
    pub fn num_splits_in_direction(&self, axis: SplitDirection) -> usize {
        match self {
            PaneNode::Branch(branch) if branch.axis == axis => branch.nodes.len(),
            _ => 1,
        }
    }

    fn remove(&mut self, pane_id: PaneId) -> bool {
        match self {
            // Leaves can only be removed from the containing branch
            PaneNode::Leaf(_) => false,
            PaneNode::Branch(branch) => match branch.remove(pane_id) {
                BranchRemoveResult::NotFound => false,
                BranchRemoveResult::Removed => true,
                BranchRemoveResult::Collapse(last_node) => {
                    *self = last_node;
                    true
                }
            },
        }
    }

    fn insert(&mut self, new_pane_id: PaneId, direction: Direction) {
        match self {
            PaneNode::Leaf(old_pane_id) => {
                *self =
                    PaneNode::Branch(PaneBranch::for_leaves(*old_pane_id, new_pane_id, direction));
            }
            PaneNode::Branch(branch) => branch.insert(new_pane_id, direction),
        }
    }

    fn pane_ids(&self) -> Vec<PaneId> {
        match self {
            PaneNode::Leaf(pane) => vec![*pane],
            PaneNode::Branch(branch) => branch.get_children(),
        }
    }

    /// Find the first panes in the given direction inside of this pane.
    fn first_panes_in_direction(&self, direction: Direction) -> HashSet<PaneId> {
        match self {
            // If this is a leaf, then this is the first pane from any direction.
            PaneNode::Leaf(id) => HashSet::from([*id]),
            PaneNode::Branch(branch) => {
                // If the direction matches the split axis, then we only search the first sub-tree in the given direction.
                //  --------------------     The first panes from the left are 1 and 3.
                //  |   1     |    2   |     The first panes from the right are 2 and 3.
                //  --------------------     For these cases we must search both sub-trees.
                //  |        3         |
                //  --------------------     The first pane from down is 3.  We only need to search the first sub-tree.
                if branch.axis() == direction.axis() {
                    match direction {
                        Direction::Left | Direction::Up => branch
                            .nodes
                            .last()
                            .expect("PaneGroup has no nodes when moving focus.")
                            .1
                            .first_panes_in_direction(direction),
                        Direction::Right | Direction::Down => branch
                            .nodes
                            .first()
                            .expect("PaneBranch has no nodes when moving focus.")
                            .1
                            .first_panes_in_direction(direction),
                    }
                } else {
                    branch
                        .nodes
                        .iter()
                        .flat_map(|(_, node)| node.first_panes_in_direction(direction))
                        .collect()
                }
            }
        }
    }

    /// Sum this [`PaneNode`]s [`PaneFlex`] values along the given `axis`. Return the
    /// [`DEFAULT_FLEX_SIZE`] if this [`PaneNode`] isn't a [`PaneNode::Branch`] in the given
    /// [`SplitDirection`] (or it is a [`PaneNode::Leaf`]).
    pub fn pane_flex_sum_along_axis(&self, axis: SplitDirection) -> f32 {
        match self {
            PaneNode::Branch(pane_branch) if pane_branch.axis == axis => pane_branch
                .nodes
                .iter()
                .fold(0., |sum, (pane_flex, _)| sum + pane_flex.0),
            _ => DEFAULT_FLEX_VALUE,
        }
    }

    fn contains_pane(&self, pane_id: PaneId) -> bool {
        match self {
            PaneNode::Leaf(id) => *id == pane_id,
            PaneNode::Branch(branch) => branch.contains_pane(pane_id),
        }
    }

    fn replace_pane(&mut self, old_pane_id: PaneId, new_pane_id: PaneId) -> bool {
        match self {
            PaneNode::Leaf(id) => {
                if *id == old_pane_id {
                    *id = new_pane_id;
                    true
                } else {
                    false
                }
            }
            PaneNode::Branch(branch) => branch.replace_pane(old_pane_id, new_pane_id),
        }
    }
}

impl FindPaneByDirection for PaneNode {
    fn panes_by_direction(
        &self,
        pane_id: PaneId,
        direction: Direction,
    ) -> FindPaneByDirectionResult {
        match self {
            PaneNode::Leaf(id) => {
                if *id == pane_id {
                    FindPaneByDirectionResult::Located
                } else {
                    FindPaneByDirectionResult::NotFound
                }
            }
            PaneNode::Branch(branch) => branch.panes_by_direction(pane_id, direction),
        }
    }
}

impl PaneBranch {
    fn new(old_pane: PaneNode, new_pane: PaneNode, direction: Direction) -> Self {
        let axis = direction.axis();
        PaneBranch {
            axis,
            nodes: match direction {
                Direction::Left | Direction::Up => {
                    vec![(DEFAULT_FLEX_SIZE, new_pane), (DEFAULT_FLEX_SIZE, old_pane)]
                }
                Direction::Right | Direction::Down => {
                    vec![(DEFAULT_FLEX_SIZE, old_pane), (DEFAULT_FLEX_SIZE, new_pane)]
                }
            },
            dividers: vec![Divider::new()],
        }
    }

    /// Construct a branch that contains two leaves.
    fn for_leaves(old_leaf: PaneId, new_leaf: PaneId, direction: Direction) -> Self {
        Self::new(
            PaneNode::Leaf(old_leaf),
            PaneNode::Leaf(new_leaf),
            direction,
        )
    }

    fn split(&mut self, old_pane: PaneId, new_pane: PaneId, direction: Direction) -> bool {
        for (idx, (_, node)) in self.nodes.iter_mut().enumerate() {
            match node {
                PaneNode::Branch(branch) => {
                    if branch.split(old_pane, new_pane, direction) {
                        return true;
                    }
                }
                PaneNode::Leaf(pane) => {
                    if *pane == old_pane {
                        // If the split comes in the same direction as the previous splits
                        // on this sub-tree, we can insert the new pane into the nodes directly
                        if direction.axis() == self.axis {
                            self.nodes.insert(
                                match direction {
                                    Direction::Left | Direction::Up => idx,
                                    Direction::Right | Direction::Down => idx + 1,
                                },
                                (DEFAULT_FLEX_SIZE, PaneNode::Leaf(new_pane)),
                            );
                            self.dividers.insert(idx, Divider::new());
                        } else {
                            // Otherwise, split the current leaf into a perpendicular branch
                            *node = PaneNode::Branch(PaneBranch::for_leaves(
                                *pane, new_pane, direction,
                            ));
                        }
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Inserts `new_pane_id` into this branch at either the start or the end, according to the
    /// [`Direction`]. If the direction axis does not match that of this branch, the branch is
    /// re-split in place.
    fn insert(&mut self, new_pane_id: PaneId, direction: Direction) {
        if direction.axis() == self.axis {
            match direction {
                Direction::Left | Direction::Up => {
                    self.nodes
                        .insert(0, (DEFAULT_FLEX_SIZE, PaneNode::Leaf(new_pane_id)));
                    self.dividers.insert(0, Divider::new());
                }
                Direction::Right | Direction::Down => {
                    self.nodes
                        .push((DEFAULT_FLEX_SIZE, PaneNode::Leaf(new_pane_id)));
                    self.dividers.push(Divider::new());
                }
            }
        } else {
            // If the axes don't match, split this branch in place.
            let nodes = mem::take(&mut self.nodes);
            let dividers = mem::take(&mut self.dividers);
            let axis = self.axis;
            *self = PaneBranch::new(
                PaneNode::Branch(PaneBranch {
                    nodes,
                    dividers,
                    axis,
                }),
                PaneNode::Leaf(new_pane_id),
                direction,
            );
        }
    }

    fn remove(&mut self, pane_id_to_remove: PaneId) -> BranchRemoveResult {
        for (idx, (_, node)) in self.nodes.iter_mut().enumerate() {
            match node {
                PaneNode::Branch(_) => {
                    if node.remove(pane_id_to_remove) {
                        return BranchRemoveResult::Removed;
                    }
                }
                PaneNode::Leaf(pane) => {
                    if *pane == pane_id_to_remove {
                        self.nodes.remove(idx);
                        if self.dividers.is_empty() {
                            log::error!("Attempted to remove a pane when there are no dividers!");
                        } else {
                            self.dividers.remove(idx.min(self.dividers.len() - 1));
                        }
                        if self.nodes.len() == 1 {
                            // Safety: We know that there is an element in `self.nodes`
                            return BranchRemoveResult::Collapse(self.nodes.pop().unwrap().1);
                        } else {
                            return BranchRemoveResult::Removed;
                        }
                    }
                }
            }
        }

        BranchRemoveResult::NotFound
    }

    pub fn get_children(&self) -> Vec<PaneId> {
        let mut res = vec![];
        for (_, member) in &self.nodes {
            match member {
                PaneNode::Branch(branch) => res.extend(branch.get_children()),
                PaneNode::Leaf(leaf) => res.push(*leaf),
            }
        }
        res
    }

    pub fn axis(&self) -> SplitDirection {
        self.axis
    }

    // Find the sibling of the given pane in the given direction.
    //  They must be direct children of the same branch.
    pub fn sibling_by_direction(&self, pane_id: PaneId, direction: Direction) -> Option<PaneId> {
        for (idx, (_, node)) in self.nodes.iter().enumerate() {
            match node {
                PaneNode::Branch(branch) => {
                    if let Some(id) = branch.sibling_by_direction(pane_id, direction) {
                        return Some(id);
                    }
                }
                PaneNode::Leaf(id) => {
                    if direction.axis() == self.axis() && *id == pane_id {
                        return match direction {
                            Direction::Left | Direction::Up => {
                                if idx == 0 {
                                    None
                                } else {
                                    match &self.nodes[idx - 1].1 {
                                        PaneNode::Leaf(id) => Some(*id),
                                        _ => None,
                                    }
                                }
                            }
                            Direction::Right | Direction::Down => {
                                if idx == self.nodes.len() - 1 {
                                    None
                                } else {
                                    match &self.nodes[idx + 1].1 {
                                        PaneNode::Leaf(id) => Some(*id),
                                        _ => None,
                                    }
                                }
                            }
                        };
                    }
                }
            }
        }
        None
    }

    fn contains_pane(&self, pane_id: PaneId) -> bool {
        self.nodes
            .iter()
            .any(|(_, node)| node.contains_pane(pane_id))
    }

    fn replace_pane(&mut self, old_pane_id: PaneId, new_pane_id: PaneId) -> bool {
        for (_, node) in &mut self.nodes {
            if node.replace_pane(old_pane_id, new_pane_id) {
                return true;
            }
        }
        false
    }

    pub fn has_visible_children(&self, hidden_panes: &[HiddenPane]) -> bool {
        self.nodes
            .iter()
            .any(|(_, node)| node.has_visible_children(hidden_panes))
    }

    pub fn has_children_hidden_for_move(&self, hidden_panes: &[HiddenPane]) -> bool {
        self.nodes
            .iter()
            .any(|(_, node)| node.has_children_hidden_for_move(hidden_panes))
    }
}

fn pane_hidden_for_job(hidden_panes: &[HiddenPane], id: &PaneId) -> bool {
    hidden_panes
        .iter()
        .any(|pane| pane.reason == HiddenPaneReason::FromJob && pane.pane_id == *id)
}

pub fn pane_hidden_for_move(hidden_panes: &[HiddenPane], id: &PaneId) -> bool {
    hidden_panes
        .iter()
        .any(|pane| pane.reason == HiddenPaneReason::FromMove && pane.pane_id == *id)
}

fn pane_hidden_for_undo(hidden_panes: &[HiddenPane], id: &PaneId) -> bool {
    hidden_panes
        .iter()
        .any(|pane| pane.reason == HiddenPaneReason::Closed && pane.pane_id == *id)
}

fn pane_hidden_for_child_agent(hidden_panes: &[HiddenPane], id: &PaneId) -> bool {
    hidden_panes
        .iter()
        .any(|pane| pane.reason == HiddenPaneReason::ChildAgent && pane.pane_id == *id)
}

impl FindPaneByDirection for PaneBranch {
    fn panes_by_direction(
        &self,
        pane_id: PaneId,
        direction: Direction,
    ) -> FindPaneByDirectionResult {
        for (idx, (_, node)) in self.nodes.iter().enumerate() {
            let res = node.panes_by_direction(pane_id, direction);

            match res {
                FindPaneByDirectionResult::Found(_) => return res,
                FindPaneByDirectionResult::Located => {
                    // If the axis is different, we left for the parent branch to handle.
                    if direction.axis() != self.axis {
                        return res;
                    }

                    let target_panes = match direction {
                        Direction::Left | Direction::Up => {
                            if idx == 0 {
                                return res;
                            }
                            self.nodes[idx - 1].1.first_panes_in_direction(direction)
                        }
                        Direction::Right | Direction::Down => {
                            if idx == self.nodes.len() - 1 {
                                return res;
                            }
                            self.nodes[idx + 1].1.first_panes_in_direction(direction)
                        }
                    };

                    return FindPaneByDirectionResult::Found(target_panes);
                }
                FindPaneByDirectionResult::NotFound => (),
            }
        }
        FindPaneByDirectionResult::NotFound
    }
}

impl fmt::Debug for PaneData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Root({:?})", self.root)
    }
}

impl fmt::Debug for PaneNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaneNode::Leaf(pane) => write!(f, "Leaf({pane:?})"),
            PaneNode::Branch(branch) => write!(f, "Branch {branch:?}"),
        }
    }
}

impl fmt::Debug for PaneBranch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.axis {
            SplitDirection::Horizontal => write!(f, "Horizontal({:?})", self.nodes),
            SplitDirection::Vertical => write!(f, "Vertical({:?})", self.nodes),
        }
    }
}
// 原 app 侧 #[cfg(test)] 访问器 (tree_tests.rs 依赖), 随类型下沉为常驻 API:
// 依赖 crate 编译时无 cfg(test), 测试 impl 必须在 lib 本体可见。
impl PaneNode {
    pub fn as_branch(&self) -> Option<&PaneBranch> {
        match self {
            PaneNode::Branch(branch) => Some(branch),
            PaneNode::Leaf(_) => None,
        }
    }

    pub fn as_leaf(&self) -> Option<PaneId> {
        match self {
            PaneNode::Leaf(id) => Some(*id),
            PaneNode::Branch(_) => None,
        }
    }
}

impl PaneBranch {
    /// 叶子 panes that are direct children of this branch。
    pub fn direct_children(&self) -> Vec<PaneId> {
        self.nodes
            .iter()
            .filter_map(|(_, node)| match node {
                PaneNode::Leaf(id) => Some(*id),
                PaneNode::Branch(_) => None,
            })
            .collect()
    }

    /// Returns a reference to the child node at `index`, panicking if it's out of bounds.
    pub fn node(&self, index: usize) -> &PaneNode {
        let (_, node) = &self.nodes[index];
        node
    }

    /// tree_tests.rs 兼容入口 (原 app 侧 private 方法, 同名透传)。
    pub fn sibling_by_direction_test(
        &self,
        pane_id: PaneId,
        direction: Direction,
    ) -> Option<PaneId> {
        self.sibling_by_direction(pane_id, direction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 幂等锁定: 纯树算法在下沉后行为不变 (split/pane_ids/remove/has_horizontal_split)。
    #[test]
    fn split_and_ids_roundtrip() {
        let panes = [
            PaneId::dummy_pane_id(),
            PaneId::dummy_pane_id(),
            PaneId::dummy_pane_id(),
        ];
        let [a, b, c] = panes;
        let mut tree = PaneData::new(a);
        assert!(tree.split(a, b, Direction::Right));
        assert!(tree.split(a, c, Direction::Down));
        assert_eq!(tree.pane_ids(), vec![a, c, b]);
        assert!(tree.has_horizontal_split());
        assert!(tree.remove(b));
        assert_eq!(tree.pane_ids(), vec![a, c]);
        assert!(!tree.has_horizontal_split());
    }
}
