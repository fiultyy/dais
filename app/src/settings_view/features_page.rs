//! `render_group` 等页面小工具。原属已删除的 `features_page.rs`(7246 行整体裁切),
//! 仅保留仍被 `features/` 子模块引用的渲染助手,避免为 3 个调用点复活整个页面。

use warpui::elements::{
    ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty, Flex, MainAxisSize,
    ParentElement, Radius, Shrinkable,
};
use warpui::Element;

use super::settings_page::HEADER_PADDING;
use crate::appearance::Appearance;

pub(super) fn render_group(
    children: impl IntoIterator<Item = Box<dyn Element>>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let bar = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(4.)
            .finish(),
    )
    .with_background(appearance.theme().outline())
    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
    .with_margin_right(8.)
    .with_margin_left(8.)
    .finish();

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(bar)
            .with_child(
                Shrinkable::new(1., Flex::column().with_children(children).finish()).finish(),
            )
            .finish(),
    )
    .with_margin_top(-4.)
    .with_margin_bottom(HEADER_PADDING)
    .finish()
}
