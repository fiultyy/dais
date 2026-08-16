use super::*;
use crate::{
    elements::{
        ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult,
        DragBarSide, Empty, EventHandler, Expanded, Flex, MainAxisSize, OffsetPositioning,
        ParentAnchor, ParentElement, ParentOffsetBounds, Rect, Resizable, ScrollStateHandle,
        Scrollable, ScrollbarWidth, Shrinkable, Stack, Fill,
    },
    platform::WindowStyle,
    App, AppContext, Entity, Presenter, TypedActionView, ViewContext, WindowInvalidation,
};
use pathfinder_geometry::vector::vec2f;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum ElementIdentifier {
    Base,
    Inset,
    Overlay,
}

#[derive(Default)]
struct View {
    // Maps identifier to number of mouse down events
    mouse_downs: HashMap<ElementIdentifier, usize>,
    list_state: UniformListState,
}

pub fn init(app: &mut AppContext) {
    app.add_action("event_handler_test:mouse_down", View::mouse_down);
}

impl View {
    fn mouse_down(&mut self, identifier: &ElementIdentifier, _: &mut ViewContext<Self>) -> bool {
        let entry = self.mouse_downs.entry(*identifier).or_insert(0);
        *entry += 1;
        true
    }
}

impl Entity for View {
    type Event = ();
}

impl crate::core::View for View {
    fn ui_name() -> &'static str {
        "event_handler_test_view"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        UniformList::new(self.list_state.clone(), 1, move |_, _| {
            let mut inner_stack = Stack::new();
            inner_stack.add_child(
                ConstrainedBox::new(Rect::new().finish())
                    .with_height(100.)
                    .with_width(100.)
                    .finish(),
            );
            inner_stack.add_positioned_child(
                EventHandler::new(
                    ConstrainedBox::new(Rect::new().finish())
                        .with_height(25.)
                        .with_width(25.)
                        .finish(),
                )
                .on_left_mouse_down(|evt, _, _| {
                    evt.dispatch_action("event_handler_test:mouse_down", ElementIdentifier::Inset);
                    DispatchEventResult::StopPropagation
                })
                .finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 75.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );

            let mut stack = Stack::new();
            stack.add_child(
                EventHandler::new(inner_stack.finish())
                    .on_left_mouse_down(|evt, _, _| {
                        evt.dispatch_action(
                            "event_handler_test:mouse_down",
                            ElementIdentifier::Base,
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
            stack.add_positioned_child(
                EventHandler::new(
                    ConstrainedBox::new(Rect::new().finish())
                        .with_height(25.)
                        .with_width(25.)
                        .finish(),
                )
                .on_left_mouse_down(|evt, _, _| {
                    evt.dispatch_action(
                        "event_handler_test:mouse_down",
                        ElementIdentifier::Overlay,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(75., 0.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );

            [stack.finish()].into_iter()
        })
        .finish()
    }
}

impl TypedActionView for View {
    type Action = ();
}

#[test]
fn test_uniform_layered_click_handling() {
    App::test((), |mut app| async move {
        let app = &mut app;
        app.update(init);
        let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |_| View::default());

        let mut presenter = Presenter::new(window_id);

        let mut updated = HashSet::new();
        updated.insert(app.root_view_id(window_id).unwrap());
        let invalidation = WindowInvalidation {
            updated,
            ..Default::default()
        };

        app.update(move |ctx| {
            presenter.invalidate(invalidation, ctx);
            let scene = presenter.build_scene(vec2f(100., 100.), 1., None, ctx);
            assert_eq!(scene.z_index(), ZIndex::new(0));
            assert_eq!(scene.layer_count(), 6);
            let presenter = Rc::new(RefCell::new(presenter));

            // Click on the overlay
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(90., 10.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter.clone(),
            );

            // Click on the inset
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(10., 90.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter.clone(),
            );

            // Click on the top-left area of the base
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(10., 10.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter.clone(),
            );

            // Click on the bottom-right area of the base
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(90., 90.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter,
            );
        });

        view.read(app, |view, _| {
            assert_eq!(
                1,
                *view.mouse_downs.get(&ElementIdentifier::Overlay).unwrap()
            );
            assert_eq!(1, *view.mouse_downs.get(&ElementIdentifier::Inset).unwrap());
            assert_eq!(2, *view.mouse_downs.get(&ElementIdentifier::Base).unwrap());
        });
    });
}

/// Repro for observatory blocks sidebar: a resizable sidebar (col(Max) with
/// an Expanded Scrollable+UniformList) inside a Stretch row inside an Expanded
/// root column. Regression guard for scroll-wheel handling in that shape.
struct SidebarScrollView {
    sessions_list: UniformListState,
    blocks_list: UniformListState,
    sessions_scroll_state: ScrollStateHandle,
    blocks_scroll_state: ScrollStateHandle,
    resize_state: crate::elements::ResizableStateHandle,
}

impl SidebarScrollView {
    fn scrolled_list(
        &self,
        scroll_state: ScrollStateHandle,
        list_state: UniformListState,
        count: usize,
    ) -> Box<dyn Element> {
        let list = UniformList::new(list_state, count, move |_, _| {
            [ConstrainedBox::new(Empty::new().finish())
                .with_height(38.)
                .finish()]
            .into_iter()
        });
        let scrollable = Scrollable::vertical(
            scroll_state,
            list.finish_scrollable(),
            ScrollbarWidth::Auto,
            Fill::None,
            Fill::None,
            Fill::None,
        )
        .with_overlayed_scrollbar();
        scrollable.finish()
    }

    fn blocks_sidebar(&self) -> Box<dyn Element> {
        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(8.);
        col.add_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(24.)
                .finish(),
        );
        col.add_child(
            Expanded::new(
                1.,
                self.scrolled_list(
                    self.blocks_scroll_state.clone(),
                    self.blocks_list.clone(),
                    50,
                ),
            )
            .finish(),
        );
        let mut raw_col = Flex::column().with_main_axis_size(MainAxisSize::Min);
        raw_col.add_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(16.)
                .finish(),
        );
        raw_col.add_child(
            ConstrainedBox::new(self.scrolled_list(
                ScrollStateHandle::default(),
                UniformListState::new(),
                20,
            ))
            .with_max_height(160.)
            .finish(),
        );
        col.add_child(raw_col.finish());

        let sidebar = Container::new(
            ConstrainedBox::new(col.finish())
                .with_min_width(240.)
                .finish(),
        )
        .finish();
        Resizable::new(self.resize_state.clone(), sidebar)
            .with_dragbar_side(DragBarSide::Left)
            .with_bounds_callback(Box::new(|window_size| {
                let min = 240.;
                let max = (window_size.x() * 0.4).max(min);
                (min, max)
            }))
            .finish()
    }
}

impl Entity for SidebarScrollView {
    type Event = ();
}

impl crate::core::View for SidebarScrollView {
    fn ui_name() -> &'static str {
        "sidebar_scroll_test_view"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        let mut main_col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.);
        main_col.add_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(24.)
                .finish(),
        );
        main_col.add_child(
            Expanded::new(
                1.,
                self.scrolled_list(
                    self.sessions_scroll_state.clone(),
                    self.sessions_list.clone(),
                    100,
                ),
            )
            .finish(),
        );

        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(Shrinkable::new(1., main_col.finish()).finish())
            .with_child(self.blocks_sidebar())
            .finish();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(8.);
        col.add_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(20.)
                .finish(),
        );
        col.add_child(
            Expanded::new(1., ConstrainedBox::new(row).with_max_height(1600.).finish()).finish(),
        );
        col.finish()
    }
}

impl TypedActionView for SidebarScrollView {
    type Action = ();
}

#[test]
fn test_scroll_wheel_in_resizable_sidebar() {
    App::test((), |mut app| async move {
        let (window_id, view) =
            app.add_window(WindowStyle::NotStealFocus, |_| SidebarScrollView {
                sessions_list: UniformListState::new(),
                blocks_list: UniformListState::new(),
                sessions_scroll_state: ScrollStateHandle::default(),
                blocks_scroll_state: ScrollStateHandle::default(),
                resize_state: crate::elements::resizable_state_handle(320.),
            });

        let mut presenter = Presenter::new(window_id);
        let mut updated = HashSet::new();
        updated.insert(app.root_view_id(window_id).unwrap());
        let invalidation = WindowInvalidation {
            updated,
            ..Default::default()
        };

        let presenter = Rc::new(RefCell::new(presenter));
        let view_inner = view.clone();
        app.update({
            let presenter = presenter.clone();
            move |ctx| {
                presenter.borrow_mut().invalidate(invalidation, ctx);
                let _ = presenter
                    .borrow_mut()
                    .build_scene(vec2f(900., 600.), 1., None, ctx);

                // Reproduce the production "scroll to latest on open" flow:
                // scroll_to(last) is consumed by the next layout pass, then
                // the user wheels up (positive delta).
                view_inner.update(ctx, |v, _| v.blocks_list.scroll_to(49));
                let _ = presenter
                    .borrow_mut()
                    .build_scene(vec2f(900., 600.), 1., None, ctx);
                let scrolled_to_bottom =
                    view_inner.read(ctx, |v, _| v.blocks_list.scroll_top());
                assert!(
                    scrolled_to_bottom.as_f64() > 0.,
                    "scroll_to(latest) should have scrolled to the bottom, got {}",
                    scrolled_to_bottom.as_f64()
                );

                // Wheel up over the blocks sidebar list (right side of the
                // window, inside the 320px-wide sidebar starting at x=580).
                ctx.simulate_window_event(
                    Event::ScrollWheel {
                        position: vec2f(740., 234.),
                        delta: vec2f(0., 3.),
                        precise: false,
                        modifiers: Default::default(),
                    },
                    window_id,
                    presenter.clone(),
                );
            }
        });

        view.read(&app, |v, _| {
            let top = v.blocks_list.scroll_top().as_f64();
            assert!(
                top > 0.,
                "blocks sidebar list should still be scrollable after scroll_to(latest) + wheel up, got {}",
                top
            );
        });
    });
}
