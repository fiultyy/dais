//! 树类型宿主耦合层 (pane_tree v2: 纯类型/纯算法已下沉 crates/pane_tree/src/tree.rs)。
//!
//! 本文件只保留依赖 PaneGroup 宿主/view 层的方法 (render/render_pane_id/
//! element_position/ViewContext<PaneGroup>), 通过 extension trait 挂回
//! `pane_tree::tree` 的类型。纯算法 (split/remove/move/hidden panes/方向查找)
//! 一律在 pane_tree::tree。

use pathfinder_geometry::vector::Vector2F;
use warpui::elements::{
    ChildAnchor, Container, DispatchEventResult, Empty, OffsetPositioning, ParentAnchor,
    ParentOffsetBounds, PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition,
    Stack,
};
use warpui::AppContext;
use warpui::{
    elements::{
        ConstrainedBox, Element, EventHandler, Flex, Hoverable, ParentElement,
        Rect, Shrinkable,
    },
    platform::Cursor,
    EntityId, ViewContext,
};

// 类型真身在 pane_tree::tree; 此处 pub use 让 crate::pane_group::tree::X
// 与 `pub use tree::{...}` (mod.rs) 继续解析。
pub use pane_tree::tree::{
    Direction, FindPaneByDirection, FindPaneByDirectionResult, HiddenPane, HiddenPaneReason,
    PaneBranch, PaneData, PaneNode,
};
use pane_tree::tree::{pane_hidden_for_move, Divider};
use super::{ActivationReason, PaneGroup, PaneId};
use crate::pane_group::{get_minimum_pane_size, DraggedBorder, PaneGroupAction};
use crate::themes::theme::WarpTheme;
use warp_core::features::FeatureFlag;

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;

pub use pane_tree::tree::{DEFAULT_FLEX_SIZE, DEFAULT_FLEX_VALUE};
pub use pane_tree::snapshot::{PaneFlex, SplitDirection};

pub fn get_divider_thickness() -> f32 {
    if FeatureFlag::MinimalistUI.is_enabled() {
        1.0
    } else {
        2.0
    }
}

// Extra padding for the divider to make it easier to resize.
// This is added around each side of the divider. Only used
// when minimalist UI is enabled.
const DIVIDER_RESIZE_PADDING: f32 = 4.0;

/// Host-coupled methods on [`PaneData`] (依赖宿主渲染/元素定位, 留守 app 侧)。
pub trait PaneDataHost {
    fn render(&self, theme: &WarpTheme, app: &AppContext) -> Box<dyn Element>;
    fn adjust_pane_size(
        &mut self,
        border_id: EntityId,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    );
    fn adjust_pane_size_by_id(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    );
    fn panes_by_direction(
        &self,
        pane_id: PaneId,
        direction: Direction,
        ctx: &ViewContext<PaneGroup>,
    ) -> Vec<PaneId>;
}

impl PaneDataHost for PaneData {
    fn render(&self, theme: &WarpTheme, app: &AppContext) -> Box<dyn Element> {
        match &self.root {
            PaneNode::Leaf(pane) => crate::pane_group::pane::render_pane_id(&pane, app),
            PaneNode::Branch(node) => node.render(theme, self.hidden_panes(), app),
        }
    }

    fn adjust_pane_size(
        &mut self,
        border_id: EntityId,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.root.adjust_pane_size(border_id, delta, ctx);
    }

    fn adjust_pane_size_by_id(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.root
            .adjust_pane_size_by_id(pane_id, direction, delta, ctx);
    }

    fn panes_by_direction(
        &self,
        pane_id: PaneId,
        direction: Direction,
        ctx: &ViewContext<PaneGroup>,
    ) -> Vec<PaneId> {
        // Find the panes from the current pane in the given direction.
        // Due to uneven splits, we may have multiple panes in the same direction.
        // Detect which ones are touching the current pane by checking the boundaries from the view context.

        if let FindPaneByDirectionResult::Found(ids) =
            self.root.panes_by_direction(pane_id, direction)
        {
            if let Some(current_rect) =
                ctx.element_position_by_id(crate::pane_group::pane::pane_position_id(&pane_id))
            {
                ids.into_iter()
                    .filter(|id| {
                        match ctx
                            .element_position_by_id(crate::pane_group::pane::pane_position_id(id))
                        {
                            Some(candidate_rect) => PaneData::are_rects_overlapping(
                                &current_rect,
                                &candidate_rect,
                                direction.axis(),
                            ),
                            None => true, // If we can't find the position, we assume it's overlapping
                        }
                    })
                    .collect()
            } else {
                Vec::from_iter(ids)
            }
        } else {
            // We didn't find any panes in that direction, return an empty list
            Vec::new()
        }
    }
}

/// Host-coupled methods on [`PaneNode`] (渲染/尺寸依赖宿主 view 层)。
pub trait PaneNodeHost {
    fn render(
        &self,
        theme: &WarpTheme,
        hidden_panes: &[HiddenPane],
        app: &AppContext,
    ) -> Box<dyn Element>;
    fn pane_size(&self, ctx: &mut ViewContext<PaneGroup>) -> Vector2F;
    fn adjust_pane_size(
        &mut self,
        border_id: EntityId,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool;
    fn adjust_pane_size_by_id(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool;
}

impl PaneNodeHost for PaneNode {
    fn render(
        &self,
        theme: &WarpTheme,
        hidden_panes: &[HiddenPane],
        app: &AppContext,
    ) -> Box<dyn Element> {
        match self {
            PaneNode::Leaf(view) => {
                let view = *view;
                EventHandler::new(crate::pane_group::pane::render_pane_id(&view, app))
                    .on_left_mouse_down(move |ctx, _, _| {
                        ctx.dispatch_typed_action(PaneGroupAction::Activate(
                            view,
                            ActivationReason::Click,
                        ));
                        DispatchEventResult::StopPropagation
                    })
                    .finish()
            }
            PaneNode::Branch(branch) => branch.render(theme, hidden_panes, app),
        }
    }

    fn pane_size(&self, ctx: &mut ViewContext<PaneGroup>) -> Vector2F {
        match self {
            PaneNode::Leaf(pane) => ctx
                .element_position_by_id(crate::pane_group::pane::pane_position_id(pane))
                .map_or(Vector2F::zero(), |rect| rect.size()),
            PaneNode::Branch(branch) => branch.size(ctx),
        }
    }

    fn adjust_pane_size(
        &mut self,
        border_id: EntityId,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool {
        match self {
            PaneNode::Leaf(_) => false,
            PaneNode::Branch(branch) => branch.adjust_pane_size(border_id, delta, ctx),
        }
    }

    /// The boolean value returned here indicates whether a resizing needs to
    /// be handled at a parent branch. For a leaf node, if the pane's id does not match,
    /// we returns false as its parent branch does not need to handle the resize.
    /// If it does match, we returns true so its parent branch will handle it.
    /// For a branch node, if the direction we are resizing does not match the branch
    /// axis, it will return true so a parent branch that does match will handle the
    /// resize.
    fn adjust_pane_size_by_id(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool {
        match self {
            PaneNode::Leaf(id) => *id == pane_id,
            PaneNode::Branch(branch) => {
                branch.adjust_pane_size_by_id(pane_id, direction, delta, ctx)
            }
        }
    }
}

/// Host-coupled methods on [`PaneBranch`] (渲染/尺寸/resize 依赖宿主 view 层)。
pub trait PaneBranchHost {
    fn render(
        &self,
        theme: &WarpTheme,
        hidden_panes: &[HiddenPane],
        app: &AppContext,
    ) -> Box<dyn Element>;
    fn adjust_pane_size(
        &mut self,
        border_id: EntityId,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool;
    fn size(&self, ctx: &mut ViewContext<PaneGroup>) -> Vector2F;
    fn adjust_pane_size_by_id(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool;
}

impl PaneBranchHost for PaneBranch {
    fn render(
        &self,
        theme: &WarpTheme,
        hidden_panes: &[HiddenPane],
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut parent = match self.axis {
            SplitDirection::Horizontal => Flex::row(),
            SplitDirection::Vertical => Flex::column(),
        };

        // Iterate through all the panes, skipping nodes that have no visible children
        // except when children are hidden for move operations (we need empty drop targets)
        let mut dividers = self.dividers.iter();

        // Collect divider positions to render them as positioned elements later.
        let mut divider_positions = Vec::new();

        for (flex, node) in self.nodes.iter() {
            // Skip nodes that have no visible children, but preserve nodes with children
            // hidden for move operations as they serve as drop targets
            if !node.has_visible_children(hidden_panes)
                && !node.has_children_hidden_for_move(hidden_panes)
            {
                continue;
            }
            let mut flex_value = flex.0;
            if let PaneNode::Leaf(id) = node {
                // If the pane is hidden for a move, render a divider, but set the
                // child element's flex value to 0 to skip rendering the pane's contents.
                if pane_hidden_for_move(hidden_panes, id) {
                    flex_value = 0.;
                }
            }

            parent.add_child(
                Shrinkable::new(flex_value, node.render(theme, hidden_panes, app)).finish(),
            );
            if let Some(divider) = dividers.next() {
                if matches!(node, PaneNode::Leaf(id) if pane_hidden_for_move(hidden_panes, id)) {
                    continue;
                }
                // Store a position index to render the actual divider at after we've rendered all pane content.
                // The reason we don't render the actual divider here is that, if we have rich content
                // (or anything that listens for a mouse click) to the right/bottom of the divider,
                // that content is rendered after the divider. Because of that, the clickbox for
                // that content takes precedence over the divider's clickbox, meaning the divider
                // is not clickable when this content is in the blocklist. To fix this, we wait to
                // render the actual divider until after we've rendered all pane content. We
                // cannot use an overlay because content like right click menus that overflow over
                // the divider should still take precedence over the divider's clickbox.
                let position_id = format!("divider_placeholder_{}", divider.id);
                divider_positions.push((divider, position_id.clone()));
                parent.add_child(create_divider_placeholder(self.axis, &position_id));
            }
        }
        let mut stack = Stack::new().with_constrain_absolute_children();
        stack.add_child(parent.finish());

        // Add actual dividers as positioned children anchored to their placeholders
        // (the reason we have to do it this way is explained in the large comment above)
        for (divider, position_id) in divider_positions {
            let divider_element = if FeatureFlag::MinimalistUI.is_enabled() {
                create_minimalist_divider(self.axis, divider, theme)
            } else {
                create_divider(self.axis, divider, theme)
            };

            stack.add_positioned_child(
                divider_element,
                OffsetPositioning::offset_from_save_position_element(
                    position_id,
                    Vector2F::new(0., 0.),
                    PositionedElementOffsetBounds::Unbounded,
                    PositionedElementAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        stack.finish()
    }

    fn adjust_pane_size(
        &mut self,
        border_id: EntityId,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool {
        if let Some(idx) = self
            .dividers
            .iter()
            .position(|divider| divider.id == border_id)
        {
            let pane_size_1 = self.nodes[idx].1.pane_size(ctx);
            let pane_size_2 = self.nodes[idx + 1].1.pane_size(ctx);

            let flex_1 = self.nodes[idx].0 .0;
            let flex_2 = self.nodes[idx + 1].0 .0;

            let total_flex = flex_1 + flex_2;

            let (size_1, size_2) = match self.axis {
                SplitDirection::Horizontal => (pane_size_1.x(), pane_size_2.x()),
                SplitDirection::Vertical => (pane_size_1.y(), pane_size_2.y()),
            };

            // Omit noise in dragging.
            let minimum_pane_size = get_minimum_pane_size(ctx);
            if size_1 + delta < minimum_pane_size
                || size_2 - delta < minimum_pane_size
                || delta.abs() < f32::EPSILON
            {
                return true;
            }

            // Re-distribute the flex factors.
            let new_flex = ((size_1 + delta) / (size_1 + size_2) * total_flex)
                .max(0.)
                .min(total_flex);

            self.nodes[idx].0 = PaneFlex(new_flex);
            self.nodes[idx + 1].0 = PaneFlex(total_flex - new_flex);

            return true;
        }

        for (_, node) in &mut self.nodes {
            if node.adjust_pane_size(border_id, delta, ctx) {
                return true;
            }
        }

        false
    }

    // Get the size of a branch by recursively adding the size of its children.
    fn size(&self, ctx: &mut ViewContext<PaneGroup>) -> Vector2F {
        match self.axis {
            SplitDirection::Horizontal => Vector2F::new(
                self.nodes
                    .iter()
                    .fold(0., |x, (_, node)| x + node.pane_size(ctx).x()),
                self.nodes[0].1.pane_size(ctx).y(),
            ),
            SplitDirection::Vertical => Vector2F::new(
                self.nodes[0].1.pane_size(ctx).x(),
                self.nodes
                    .iter()
                    .fold(0., |y, (_, node)| y + node.pane_size(ctx).y()),
            ),
        }
    }

    fn adjust_pane_size_by_id(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        delta: f32,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> bool {
        for (idx, (_, node)) in self.nodes.iter_mut().enumerate() {
            if node.adjust_pane_size_by_id(pane_id, direction, delta, ctx) {
                // If the resizing direction is different from the splitting direction
                // of the branch, we return for the parents to handle.
                if direction != self.axis {
                    return true;
                }

                let divider_id = self.dividers[idx.min(self.dividers.len() - 1)].id;
                self.adjust_pane_size(divider_id, delta, ctx);
                break;
            }
        }
        false
    }
}

/// Create an invisible placeholder element that occupies the same space as the divider
/// and saves its position so the actual divider can be anchored to it.
fn create_divider_placeholder(direction: SplitDirection, position_id: &str) -> Box<dyn Element> {
    let divider_thickness = get_divider_thickness() - 1.0;

    let placeholder = match direction {
        SplitDirection::Horizontal => ConstrainedBox::new(Empty::new().finish())
            .with_width(divider_thickness)
            .finish(),
        SplitDirection::Vertical => ConstrainedBox::new(Empty::new().finish())
            .with_height(divider_thickness)
            .finish(),
    };

    SavePosition::new(placeholder, position_id).finish()
}

fn create_divider(
    direction: SplitDirection,
    item: &Divider,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    let divider = ConstrainedBox::new(
        Rect::new()
            .with_background(theme.split_pane_border_color())
            .finish(),
    );

    let cursor_shape = match direction {
        SplitDirection::Horizontal => Cursor::ResizeLeftRight,
        SplitDirection::Vertical => Cursor::ResizeUpDown,
    };

    let border_id = item.id;

    Hoverable::new(item.mouse_state.clone(), |_| {
        EventHandler::new(match direction {
            SplitDirection::Horizontal => divider.with_width(get_divider_thickness()).finish(),
            SplitDirection::Vertical => divider.with_height(get_divider_thickness()).finish(),
        })
        .on_left_mouse_down(move |ctx, _, position| {
            ctx.dispatch_typed_action(PaneGroupAction::StartResizing(DraggedBorder {
                border_id,
                direction,
                previous_mouse_location: position,
            }));
            DispatchEventResult::StopPropagation
        })
        .finish()
    })
    .with_cursor(cursor_shape)
    .with_propagate_drag()
    .finish()
}

fn create_minimalist_divider(
    direction: SplitDirection,
    item: &Divider,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    let divider = ConstrainedBox::new(
        Rect::new()
            .with_background(theme.split_pane_border_color())
            .finish(),
    );

    let cursor_shape = match direction {
        SplitDirection::Horizontal => Cursor::ResizeLeftRight,
        SplitDirection::Vertical => Cursor::ResizeUpDown,
    };

    let border_id = item.id;
    let hoverable = Hoverable::new(item.mouse_state.clone(), |_| {
        let container = match direction {
            SplitDirection::Horizontal => {
                Container::new(divider.with_width(get_divider_thickness()).finish())
                    .with_padding_left(DIVIDER_RESIZE_PADDING)
                    .with_padding_right(DIVIDER_RESIZE_PADDING)
                    .finish()
            }
            SplitDirection::Vertical => {
                Container::new(divider.with_height(get_divider_thickness()).finish())
                    .with_padding_top(DIVIDER_RESIZE_PADDING)
                    .with_padding_bottom(DIVIDER_RESIZE_PADDING)
                    .finish()
            }
        };
        EventHandler::new(container)
            .on_left_mouse_down(move |ctx, _, position| {
                ctx.dispatch_typed_action(PaneGroupAction::StartResizing(DraggedBorder {
                    border_id,
                    direction,
                    previous_mouse_location: position,
                }));
                DispatchEventResult::StopPropagation
            })
            .finish()
    })
    .with_cursor(cursor_shape)
    .with_propagate_drag();

    let mut stack = Stack::new().with_constrain_absolute_children();
    match direction {
        SplitDirection::Horizontal => stack.add_positioned_child(
            hoverable.finish(),
            OffsetPositioning::offset_from_parent(
                Vector2F::new(0., 0.),
                ParentOffsetBounds::Unbounded,
                ParentAnchor::TopMiddle,
                ChildAnchor::TopMiddle,
            ),
        ),
        SplitDirection::Vertical => stack.add_positioned_child(
            hoverable.finish(),
            OffsetPositioning::offset_from_parent(
                Vector2F::new(0., -DIVIDER_RESIZE_PADDING),
                ParentOffsetBounds::Unbounded,
                ParentAnchor::TopLeft,
                ChildAnchor::TopLeft,
            ),
        ),
    };
    stack.finish()
}

// When pane group is split horizontally, new panes are added from left to right.
// When pane group is split vertically, new panes are added from top to bottom.
// SplitDirection 真身在 pane_tree::snapshot (v2 快照骨架统一类型), 此处 re-export。
impl From<crate::launch_configs::launch_config::SplitDirection> for SplitDirection {
    fn from(direction: crate::launch_configs::launch_config::SplitDirection) -> Self {
        match direction {
            crate::launch_configs::launch_config::SplitDirection::Horizontal => {
                SplitDirection::Horizontal
            }
            crate::launch_configs::launch_config::SplitDirection::Vertical => {
                SplitDirection::Vertical
            }
        }
    }
}
