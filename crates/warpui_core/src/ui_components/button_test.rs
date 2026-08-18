use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::{vec2f, Vector2F};

use super::*;
use crate::ui_components::components::UiComponentStyles;
use crate::{
    elements::{
        ClippedScrollable, ClippedScrollStateHandle, ConstrainedBox, Container, Empty, Flex,
        ScrollbarWidth, Text, UniformList, UniformListState,
    },
    App, AppContext, Element, Entity, Event, Presenter, TypedActionView, View, WindowId,
    WindowInvalidation,
};

/// 记录每次 on_click 触发的位置。
type ClickLog = Rc<RefCell<Vec<Vector2F>>>;

fn button_styles() -> UiComponentStyles {
    // test platform 的 FontDB 对任意 family 返回 FamilyId(0); Span 会 unwrap 这个字段。
    UiComponentStyles {
        height: Some(32.),
        background: Some(ColorU::new(230, 180, 80, 255).into()),
        font_family_id: Some(crate::fonts::FamilyId(0)),
        font_size: Some(14.),
        ..Default::default()
    }
}

/// 无显式 width 的 CenteredText Button 直接作为 Flex column 子元素。
#[derive(Clone)]
struct CenteredButtonView {
    clicks: ClickLog,
}

impl Entity for CenteredButtonView {
    type Event = ();
}

impl TypedActionView for CenteredButtonView {
    type Action = ();
}

impl View for CenteredButtonView {
    fn render<'a>(&self, _: &AppContext) -> Box<dyn Element> {
        let clicks = self.clicks.clone();
        let button = Button::new(
            Arc::new(Mutex::new(Default::default())),
            button_styles(),
            None,
            None,
            None,
        )
        .with_centered_text_label("CLICK ME".into())
        .build()
        .on_click(move |_, _, position| {
            clicks.borrow_mut().push(position);
        })
        .finish();

        Flex::column().with_child(button).finish()
    }

    fn ui_name() -> &'static str {
        "CenteredButtonView"
    }
}

/// 完整复刻 showcase L4 的滚动组合:
/// ClippedScrollable → [spacer, UniformList 卡(命令面板), spacer, button]。
#[derive(Clone)]
struct ScrollCaseView {
    clicks: ClickLog,
    scroll_state: ClippedScrollStateHandle,
}

impl Entity for ScrollCaseView {
    type Event = ();
}

impl TypedActionView for ScrollCaseView {
    type Action = ();
}

impl View for ScrollCaseView {
    fn render<'a>(&self, _: &AppContext) -> Box<dyn Element> {
        let clicks = self.clicks.clone();
        let button = Button::new(
            Arc::new(Mutex::new(Default::default())),
            button_styles(),
            None,
            None,
            None,
        )
        .with_centered_text_label("CLICK ME".into())
        .build()
        .on_click(move |_, _, position| {
            clicks.borrow_mut().push(position);
        })
        .finish();

        let list_items: Vec<&'static str> = vec!["a", "b", "c", "d", "e", "f"];
        let list = UniformList::new(UniformListState::new(), list_items.len(), {
            move |range, _app| {
                range
                    .into_iter()
                    .map(|i| {
                        Text::new(list_items[i].to_string(), crate::fonts::FamilyId(0), 13.)
                            .finish()
                    })
                    .collect::<Vec<Box<dyn Element>>>()
                    .into_iter()
            }
        })
        .finish();
        let list_card = Container::new(ConstrainedBox::new(list).with_height(150.).finish()).finish();

        let content = Flex::column()
            .with_child(ConstrainedBox::new(Empty::new().finish()).with_height(400.).finish())
            .with_child(list_card)
            .with_child(ConstrainedBox::new(Empty::new().finish()).with_height(400.).finish())
            .with_child(button)
            .finish();

        ClippedScrollable::vertical(
            self.scroll_state.clone(),
            content,
            ScrollbarWidth::Auto,
            crate::elements::Fill::Solid(ColorU::white()),
            crate::elements::Fill::Solid(ColorU::white()),
            crate::elements::Fill::Solid(ColorU::black()),
        )
        .finish()
    }

    fn ui_name() -> &'static str {
        "ScrollCaseView"
    }
}
/// 复刻 showcase 的卡片容器链: ConstrainedBox(max_width) + Container(padding) + Flex column。
/// loose 约束正是运行时"视觉占满卡片全宽"现象的来源。
#[derive(Clone)]
struct LooseCardView {
    clicks: ClickLog,
}

impl Entity for LooseCardView {
    type Event = ();
}

impl TypedActionView for LooseCardView {
    type Action = ();
}

impl View for LooseCardView {
    fn render<'a>(&self, _: &AppContext) -> Box<dyn Element> {
        let clicks = self.clicks.clone();
        let button = Button::new(
            Arc::new(Mutex::new(Default::default())),
            button_styles(),
            None,
            None,
            None,
        )
        .with_centered_text_label("CLICK ME".into())
        .build()
        .on_click(move |_, _, _| {
            clicks.borrow_mut().push(vec2f(0., 0.));
        })
        .finish();

        let card = Container::new(
            ConstrainedBox::new(Flex::column().with_child(button).finish())
                .with_max_width(200.)
                .finish(),
        )
        .with_uniform_padding(16.)
        .finish();

        Flex::column().with_child(card).finish()
    }

    fn ui_name() -> &'static str {
        "LooseCardView"
    }
}

fn dispatch_click(
    ctx: &mut AppContext,
    presenter: &Rc<RefCell<Presenter>>,
    window_id: WindowId,
    x: f32,
    y: f32,
) {
    for event in [
        Event::LeftMouseDown {
            position: vec2f(x, y),
            modifiers: Default::default(),
            click_count: 1,
            is_first_mouse: false,
        },
        Event::LeftMouseUp {
            position: vec2f(x, y),
            modifiers: Default::default(),
        },
    ] {
        ctx.simulate_window_event(event, window_id, presenter.clone());
    }
}

fn setup_scene(ctx: &mut AppContext, window_id: WindowId, presenter: &mut Presenter) {
    let mut updated = std::collections::HashSet::new();
    updated.insert(ctx.root_view_id(window_id).unwrap());
    let invalidation = WindowInvalidation {
        updated,
        ..Default::default()
    };
    presenter.invalidate(invalidation, ctx);
    presenter.build_scene(vec2f(300., 300.), 1., None, ctx);
}

/// 无显式 width 的 CenteredText 按钮(静态): 整个绘制宽度内点击都应触发 on_click。
/// 回归护栏: CenteredText 分支的 Align::expand 视觉宽度必须与 Hoverable 命中区一致。
#[test]
fn test_centered_text_button_hit_area_spans_painted_width() {
    App::test((), |mut app| async move {
        let clicks: ClickLog = Rc::new(RefCell::new(vec![]));
        let (window_id, _view) = app.update(|ctx| {
            ctx.add_window(crate::AddWindowOptions::default(), {
                let clicks = clicks.clone();
                move |_| CenteredButtonView { clicks }
            })
        });
        let mut presenter = Presenter::new(window_id);

        app.update(|ctx| {
            setup_scene(ctx, window_id, &mut presenter);
            let presenter = Rc::new(RefCell::new(presenter));

            for x in [10., 75., 150., 225., 290.] {
                dispatch_click(ctx, &presenter, window_id, x, 16.);
            }

            assert_eq!(
                clicks.borrow().len(),
                5,
                "整行 5 个点击点都应触发 on_click — 命中区与绘制区脱节"
            );
        });
    });
}

/// 滚动组合(scrolled + repainted): 按钮命中区仍须等于绘制区。
/// 注意: 测试 harness 不会自动重绘, 滚动后必须重建场景(真实事件循环由 notify 驱动)。
/// x=290 落在滚动条 gutter(Scrollable 消费), 其余 4 点必须全部命中。
#[test]
fn test_centered_text_button_hit_area_after_scrolling() {
    App::test((), |mut app| async move {
        let clicks: ClickLog = Rc::new(RefCell::new(vec![]));
        let scroll_state = ClippedScrollStateHandle::new();
        let (window_id, _view) = app.update(|ctx| {
            let scroll_state = scroll_state.clone();
            ctx.add_window(crate::AddWindowOptions::default(), {
                let clicks = clicks.clone();
                move |_| ScrollCaseView {
                    clicks,
                    scroll_state,
                }
            })
        });
        let mut presenter = Presenter::new(window_id);

        app.update(|ctx| {
            setup_scene(ctx, window_id, &mut presenter);
            let presenter = Rc::new(RefCell::new(presenter));

            // 滚到底(非 precise 的 delta 单位是行)
            for _ in 0..8 {
                ctx.simulate_window_event(
                    Event::ScrollWheel {
                        position: vec2f(150., 150.),
                        delta: vec2f(0., -3.),
                        precise: false,
                        modifiers: Default::default(),
                    },
                    window_id,
                    presenter.clone(),
                );
            }

            // 模拟真实事件循环的 notify → repaint
            {
                let mut p = presenter.borrow_mut();
                let mut updated = std::collections::HashSet::new();
                updated.insert(ctx.root_view_id(window_id).unwrap());
                p.invalidate(WindowInvalidation { updated, ..Default::default() }, ctx);
                p.build_scene(vec2f(300., 300.), 1., None, ctx);
            }

            let scroll_start = scroll_state.scroll_start().as_f32();
            assert!(scroll_start > 0., "预滚动应已发生, 实际 scroll_start={scroll_start}");

            // 按钮滚入视野底部(内容 400+150+400+32=982, 视口 300 → 按钮在 y≈[616+scroll, +32])
            for x in [10., 75., 150., 225.] {
                let before = clicks.borrow().len();
                dispatch_click(ctx, &presenter, window_id, x, 284.);
                assert_eq!(
                    clicks.borrow().len(),
                    before + 1,
                    "滚动后 x={x} 的点击未命中按钮(scroll_start={scroll_start}) — 命中区与绘制区脱节"
                );
            }
        });
    });
}

/// 复刻 showcase 卡片链(运行时已验证): 视口(300) → Flex column → Container(padding 16) →
/// ConstrainedBox(max_width 200) → Flex column → 无显式宽度 CenteredText 按钮。
/// loose 约束下 Align::expand 使按钮视觉占满 max_width 内的可用宽(ConstrainedBox max_width=200)。
/// 护栏: 命中区边界必须与布局(视觉)宽一致 — 扫描出的命中宽度 ≈168, 且严格落在卡片 padding 内。
/// 运行时验证记录(Xvfb 真实鼠标): 命中边界与像素扫描的视觉边界逐像素一致,
/// 此测试锁定该不变式, 防止未来 Align/Hoverable 改动引入 hit≠visual 回归。
#[test]
fn test_centered_text_button_hit_area_equals_layout_under_loose_constraint() {
    App::test((), |mut app| async move {
        let clicks: ClickLog = Rc::new(RefCell::new(vec![]));
        let (window_id, _view) = app.update(|ctx| {
            ctx.add_window(crate::AddWindowOptions::default(), {
                let clicks = clicks.clone();
                move |_| LooseCardView { clicks }
            })
        });
        let mut presenter = Presenter::new(window_id);

        app.update(|ctx| {
            setup_scene(ctx, window_id, &mut presenter);
            let presenter = Rc::new(RefCell::new(presenter));

            // y 取卡片垂直中心: 卡片高 = 按钮 32 + padding 32 = 64 → y=32
            let y = 32.;
            let mut hits: Vec<i32> = vec![];
            for x in 0..300 {
                let before = clicks.borrow().len();
                dispatch_click(ctx, &presenter, window_id, x as f32, y);
                if clicks.borrow().len() > before {
                    hits.push(x);
                }
            }
            let first = *hits.first().expect("应存在命中点");
            let last = *hits.last().expect("应存在命中点");
            let width = last - first + 1;
            assert!(
                (width as f32 - 200.).abs() <= 2.,
                "命中宽度 {width} 应等于视觉宽 200(ConstrainedBox max_width; \
                 Container padding 在其外), 边界 [{first},{last}]"
            );
            assert!(
                first >= 16 && last <= 216,
                "命中边界 [{first},{last}] 必须落在卡片 padding 圈定区 [16,216]"
            );
        });
    });
}
