//! Split-region tree for the middle content area.
//!
//! Each region owns a tab stack and renders its own tab bar; a tab binds to
//! a single view (its pane group). Splitting a region replaces its leaf with
//! a branch holding the old region and a freshly created sibling region —
//! split areas are therefore independently multi-tab.
//!
//! Reuses `SplitDirection` from the pane-group tree: `Horizontal` lays
//! children out left-to-right, `Vertical` stacks them top-to-bottom.

use std::collections::BTreeSet;

use crate::pane_group::SplitDirection;

pub(crate) type RegionId = u64;

/// First region id; every workspace starts with exactly this region.
pub(crate) const ROOT_REGION_ID: RegionId = 0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RegionNode {
    Leaf(RegionId),
    Branch {
        axis: SplitDirection,
        children: Vec<RegionChild>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RegionChild {
    pub flex: f32,
    pub node: RegionNode,
}

impl RegionChild {
    fn leaf(flex: f32, id: RegionId) -> Self {
        Self {
            flex,
            node: RegionNode::Leaf(id),
        }
    }
}

impl RegionNode {
    pub(crate) fn root() -> Self {
        Self::Leaf(ROOT_REGION_ID)
    }

    pub(crate) fn collect_region_ids(&self, out: &mut BTreeSet<RegionId>) {
        match self {
            Self::Leaf(id) => {
                out.insert(*id);
            }
            Self::Branch { children, .. } => {
                for child in children {
                    child.node.collect_region_ids(out);
                }
            }
        }
    }

    pub(crate) fn contains(&self, id: RegionId) -> bool {
        match self {
            Self::Leaf(leaf_id) => *leaf_id == id,
            Self::Branch { children, .. } => children.iter().any(|c| c.node.contains(id)),
        }
    }

    /// Any leaf id, used as a fallback when a lookup target is gone.
    pub(crate) fn first_leaf(&self) -> RegionId {
        match self {
            Self::Leaf(id) => *id,
            Self::Branch { children, .. } => children
                .first()
                .map(|c| c.node.first_leaf())
                .unwrap_or(ROOT_REGION_ID),
        }
    }

    /// Replaces `Leaf(target)` with `Branch { axis, [target, new_id] }` (or
    /// `[new_id, target]` when `new_first`), each child at half the original
    /// flex. Returns `false` when `target` is not a leaf of this tree.
    pub(crate) fn split_leaf(
        &mut self,
        target: RegionId,
        axis: SplitDirection,
        new_id: RegionId,
        new_first: bool,
    ) -> bool {
        match self {
            Self::Leaf(id) => {
                if *id != target {
                    return false;
                }
                let (first, second) = if new_first {
                    (new_id, target)
                } else {
                    (target, new_id)
                };
                *self = Self::Branch {
                    axis,
                    children: vec![
                        RegionChild::leaf(1., first),
                        RegionChild::leaf(1., second),
                    ],
                };
                true
            }
            Self::Branch {
                children,
                axis: branch_axis,
            } => {
                if *branch_axis == axis {
                    // Insert the new leaf as a direct sibling in the same
                    // axis when the target is already a leaf here: keeps the
                    // tree flat for repeated same-direction splits.
                    if let Some(position) = children
                        .iter()
                        .position(|c| matches!(c.node, RegionNode::Leaf(id) if id == target))
                    {
                        let insert_at = if new_first { position } else { position + 1 };
                        children.insert(insert_at, RegionChild::leaf(1., new_id));
                        return true;
                    }
                }
                for child in children.iter_mut() {
                    if child.node.split_leaf(target, axis, new_id, new_first) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Removes the `target` leaf and collapses any branch left with a
    /// single child (the survivor inherits the branch's slot). Returns
    /// `false` when the tree does not contain the leaf. Removing the last
    /// region of the tree is rejected — the content area always keeps at
    /// least one region.
    pub(crate) fn remove_leaf(&mut self, target: RegionId) -> bool {
        match self {
            Self::Leaf(id) => *id != ROOT_REGION_ID && *id == target,
            Self::Branch { children, .. } => {
                let before = children.len();
                children.retain(|child| !matches!(child.node, RegionNode::Leaf(id) if id == target));
                if children.len() != before {
                    Self::collapse_if_needed(self);
                    return true;
                }
                let changed = children
                    .iter_mut()
                    .any(|child| child.node.remove_leaf(target));
                if changed {
                    Self::collapse_if_needed(self);
                }
                changed
            }
        }
    }

    fn collapse_if_needed(node: &mut RegionNode) {
        if let RegionNode::Branch { children, .. } = node {
            match children.len() {
                0 => *node = RegionNode::root(),
                1 => {
                    let survivor = children.pop().expect("len checked");
                    *node = survivor.node;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_creates_sibling_branch() {
        let mut tree = RegionNode::root();
        assert!(tree.split_leaf(ROOT_REGION_ID, SplitDirection::Horizontal, 1, false));
        assert_eq!(
            tree,
            RegionNode::Branch {
                axis: SplitDirection::Horizontal,
                children: vec![
                    RegionChild::leaf(1., 0),
                    RegionChild::leaf(1., 1),
                ]
            }
        );
    }

    #[test]
    fn repeated_same_axis_splits_stay_flat() {
        let mut tree = RegionNode::root();
        tree.split_leaf(0, SplitDirection::Horizontal, 1, false);
        tree.split_leaf(1, SplitDirection::Horizontal, 2, false);
        match &tree {
            RegionNode::Branch { children, .. } => assert_eq!(children.len(), 3),
            other => panic!("expected branch, got {other:?}"),
        }
    }

    #[test]
    fn perpendicular_split_nests() {
        let mut tree = RegionNode::root();
        tree.split_leaf(0, SplitDirection::Horizontal, 1, false);
        tree.split_leaf(1, SplitDirection::Vertical, 2, false);
        match &tree {
            RegionNode::Branch { children, axis } => {
                assert_eq!(*axis, SplitDirection::Horizontal);
                assert_eq!(children.len(), 2);
                assert!(matches!(children[1].node, RegionNode::Branch { .. }));
            }
            other => panic!("expected branch, got {other:?}"),
        }
    }

    #[test]
    fn remove_leaf_collapses_branch() {
        let mut tree = RegionNode::root();
        tree.split_leaf(0, SplitDirection::Horizontal, 1, false);
        assert!(tree.remove_leaf(1));
        assert_eq!(tree, RegionNode::Leaf(0));
        // Root region is never removable.
        assert!(!tree.remove_leaf(0));
        assert_eq!(tree, RegionNode::Leaf(0));
    }

    #[test]
    fn remove_nested_leaf_keeps_siblings() {
        let mut tree = RegionNode::root();
        // 0 | (1 / 2)
        tree.split_leaf(0, SplitDirection::Horizontal, 1, false);
        tree.split_leaf(1, SplitDirection::Vertical, 2, false);
        assert!(tree.remove_leaf(1));
        match &tree {
            RegionNode::Branch { children, axis } => {
                assert_eq!(*axis, SplitDirection::Horizontal);
                assert_eq!(children.len(), 2);
                assert_eq!(children[1].node, RegionNode::Leaf(2));
            }
            other => panic!("expected branch, got {other:?}"),
        }
    }

    #[test]
    fn remove_middle_child_of_flat_branch() {
        let mut tree = RegionNode::root();
        tree.split_leaf(0, SplitDirection::Horizontal, 1, false);
        tree.split_leaf(1, SplitDirection::Horizontal, 2, false);
        assert!(tree.remove_leaf(1));
        let mut ids = BTreeSet::new();
        tree.collect_region_ids(&mut ids);
        assert_eq!(ids, BTreeSet::from([0, 2]));
    }
}
