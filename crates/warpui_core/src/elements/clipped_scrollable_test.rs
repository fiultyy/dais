use std::collections::HashSet;

use pathfinder_geometry::vector::vec2f;

use crate::{
    elements::{
        Axis, ConstrainedBox, Empty, Fill, Flex, MainAxisSize, ParentElement, SavePosition,
        ScrollbarWidth, Stack,
    },
    platform::WindowStyle,
    units::IntoPixels,
    App, Element, Entity, Presenter, TypedActionView, WindowInvalidation,
};

use super::{ClippedScrollStateHandle, ClippedScrollable, ScrollTarget, ScrollToPositionMode};

macro_rules! assert_float_eq {
    ($lhs:expr, $rhs:expr) => {{
        let lhs = $lhs;
        let rhs = $rhs;
        assert!(
            (lhs - rhs).abs() < f32::EPSILON,
            "{} ({}) != {} ({})",
            lhs,
            stringify!($lhs),
            rhs,
            stringify!($rhs)
        );
    }};
}

#[derive(Default)]
struct View {
    scroll_handle: ClippedScrollStateHandle,
}

impl Entity for View {
    type Event = ();
}

impl crate::core::View for View {
    fn ui_name() -> &'static str {
        "View"
    }

    fn render(&self, _: &crate::AppContext) -> Box<dyn crate::Element> {
        let mut children = vec![];
        for i in 0..10 {
            children.push(
                SavePosition::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_height(20.)
                        .with_width(100.)
                        .finish(),
                    &format!("child_{i}"),
                )
                .finish(),
            );
        }

        let mut stack = Stack::new();
        stack.add_child(
            ClippedScrollable::new(
                Axis::Vertical,
                Flex::column().with_children(children).finish(),
                self.scroll_handle.clone(),
            )
            .finish(),
        );
        stack.finish()
    }
}

impl TypedActionView for View {
    type Action = ();
}

#[test]
fn test_scroll_to_position() {
    App::test((), |mut app| async move {
        let app = &mut app;
        let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |_| View::default());

        let mut presenter = Presenter::new(window_id);

        let mut updated = HashSet::new();
        updated.insert(app.root_view_id(window_id).unwrap());
        let invalidation = WindowInvalidation {
            updated,
            ..Default::default()
        };

        let scroll_state = view.read(app, |view, _| view.scroll_handle.clone());
        let window_size = vec2f(100., 100.);
        let scale_factor = 1.;

        app.update(move |ctx| {
            presenter.invalidate(invalidation.clone(), ctx);
            // The `ClippedScrollable` has 10 elements in total, each with a height of 20.
            // The window height is 100 so, with a scroll top of 0, the first 5 elements should be
            // in view.
            presenter.build_scene(window_size, scale_factor, None, ctx);

            // An element fully below the scrollable area should be the last item in view
            // after we scroll to it.
            scroll_state.scroll_to_position(ScrollTarget {
                position_id: "child_6".to_string(),
                mode: ScrollToPositionMode::FullyIntoView,
            });
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);
            assert_float_eq!(scroll_state.scroll_start().as_f32(), 40.);

            // An element fully above the scrollable area should be the first item in view after
            // it's scrolled to.
            scroll_state.scroll_to_position(ScrollTarget {
                position_id: "child_1".to_string(),
                mode: ScrollToPositionMode::FullyIntoView,
            });
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);
            assert_float_eq!(scroll_state.scroll_start().as_f32(), 20.);

            // An element fully within the viewport should no-op.
            scroll_state.scroll_to_position(ScrollTarget {
                position_id: "child_3".to_string(),
                mode: ScrollToPositionMode::FullyIntoView,
            });
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);
            assert_float_eq!(scroll_state.scroll_start().as_f32(), 20.);

            // An element that is partially above the viewport should be scrolled fully within the viewport.
            // First, make the scroll top 1.0 pixels. We need to call build scene after this so the
            // position cache is updated appropriately.
            scroll_state.clipped_scroll_data.lock().scroll_start_px = (1_f32).into_pixels();
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);

            // Now we can invoke the scroll to position API and verify the correct result.
            scroll_state.scroll_to_position(ScrollTarget {
                position_id: "child_0".to_string(),
                mode: ScrollToPositionMode::FullyIntoView,
            });
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);
            assert_float_eq!(scroll_state.scroll_start().as_f32(), 0.);

            // An element that is partially below the viewport should be scrolled fully within the viewport.
            // First, make the scroll top 1.0 pixels. We need to call build scene after this so the
            // position cache is updated appropriately.
            scroll_state.clipped_scroll_data.lock().scroll_start_px = (1_f32).into_pixels();
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);

            // Now we can invoke the scroll to position API and verify the correct result.
            scroll_state.scroll_to_position(ScrollTarget {
                position_id: "child_5".to_string(),
                mode: ScrollToPositionMode::FullyIntoView,
            });
            presenter.invalidate(invalidation.clone(), ctx);
            presenter.build_scene(window_size, scale_factor, None, ctx);
            assert_float_eq!(scroll_state.scroll_start().as_f32(), 20.);
        });
    });
}

/// View whose root IS the scrollable (finite viewport: window bounds).
#[derive(Default)]
struct WheelView {
    scroll_handle: ClippedScrollStateHandle,
    /// Wrap in a `Flex::column(Min)` — reproduces the observatory sidebar
    /// bug shape where a flex parent hands its non-flex child an
    /// unconstrained main axis.
    wrap_in_min_col: bool,
}

impl Entity for WheelView {
    type Event = ();
}

impl crate::core::View for WheelView {
    fn ui_name() -> &'static str {
        "clipped_wheel_view"
    }

    fn render(&self, _: &crate::AppContext) -> Box<dyn crate::Element> {
        let mut children = vec![];
        for i in 0..10 {
            children.push(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(20.)
                    .with_width(100.)
                    .finish(),
            );
            let _ = i;
        }
        // Scrollable shell (what `ClippedScrollable::vertical` returns) owns
        // wheel dispatch; plain (non-list) content is exactly the shape the
        // observatory block-detail sidebar renders.
        let scrollable = ClippedScrollable::vertical(
            self.scroll_handle.clone(),
            Flex::column().with_children(children).finish(),
            ScrollbarWidth::Auto,
            Fill::None,
            Fill::None,
            Fill::None,
        )
        .finish();
        if self.wrap_in_min_col {
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(scrollable)
                .finish()
        } else {
            scrollable
        }
    }
}

impl TypedActionView for WheelView {
    type Action = ();
}

fn wheel_scene_and_scroll(
    app: &mut crate::App,
    wrap_in_min_col: bool,
) -> (f32, f32) {
    let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |_| WheelView {
        scroll_handle: ClippedScrollStateHandle::default(),
        wrap_in_min_col,
    });
    let scroll_state = view.read(app, |v, _| v.scroll_handle.clone());
    let mut presenter = Presenter::new(window_id);
    let mut updated = HashSet::new();
    updated.insert(app.root_view_id(window_id).unwrap());
    let invalidation = WindowInvalidation {
        updated,
        ..Default::default()
    };
    let presenter = std::rc::Rc::new(std::cell::RefCell::new(presenter));
    app.update({
        let presenter = presenter.clone();
        let scroll_state = scroll_state.clone();
        move |ctx| {
            presenter.borrow_mut().invalidate(invalidation, ctx);
            // 10 children × 20px = 200px of content in a 100px window.
            presenter
                .borrow_mut()
                .build_scene(vec2f(100., 100.), 1., None, ctx);
            let before = scroll_state.scroll_start().as_f32();
            ctx.simulate_window_event(
                crate::Event::ScrollWheel {
                    position: vec2f(50., 50.),
                    delta: vec2f(0., -3.),
                    precise: false,
                    modifiers: Default::default(),
                },
                window_id,
                presenter.clone(),
            );
            let after = scroll_state.scroll_start().as_f32();
            (before, after)
        }
    })
}

/// Positive control: with a finite viewport (root = scrollable, window
/// bounds) the wheel scrolls plain (non-list) content.
#[test]
fn test_clipped_scrollable_wheel_scrolls_plain_content_with_finite_viewport() {
    App::test((), |mut app| async move {
        let (before, after) = wheel_scene_and_scroll(&mut app, false);
        assert_float_eq!(before, 0.);
        assert!(
            after > before,
            "wheel must scroll when viewport is finite, got {before} -> {after}"
        );
    });
}

/// Root-cause pin: under an unconstrained main axis (flex `column(Min)`
/// wrapping the scrollable — the observatory block-detail sidebar shape)
/// the viewport grows to full content height, `scroll()`'s
/// `child_size > clipped_size` is never true, and the wheel is silently
/// dropped. This is why that sidebar must not be wrapped in a plain flex
/// column; give the scrollable a finite viewport instead.
#[test]
fn test_clipped_scrollable_wheel_noop_under_unconstrained_viewport() {
    App::test((), |mut app| async move {
        let (before, after) = wheel_scene_and_scroll(&mut app, true);
        let msg = "wheel must be a silent no-op under an unconstrained viewport (documented root cause)";
        assert!(
            (after - 0.).abs() < f32::EPSILON,
            "{msg}, got {before} -> {after}"
        );
    });
}
