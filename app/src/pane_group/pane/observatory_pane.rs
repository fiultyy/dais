//! 观测台 pane — ObservatoryPanelView 的标签页壳。
//!
//! 观测台业务态全在 `ObservatoryModel` 单例（快照/选中/tab/composer），
//! 本 pane 只是把面板 view 装进 pane 体系（标题、焦点、关闭）。
//! 不持久化：重启后从工具条重新打开。

use crate::pane_group::pane::PaneIdConstruct;
use warpui::{AppContext, ModelHandle, SingletonEntity, ViewContext, ViewHandle};

use crate::ai::observatory::model::ObservatoryModel;
use crate::ai::observatory::view::ObservatoryPanelView;
use crate::app_state::LeafContents;

use super::{
    view::PaneView, BackingView, DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId,
    ShareableLink, ShareableLinkError,
};

pub struct ObservatoryPane {
    view: ViewHandle<PaneView<ObservatoryPanelView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl ObservatoryPane {
    /// 在 pane group 中新建观测台 pane（同时打开面板数据轮询）。
    pub fn new(ctx: &mut ViewContext<PaneGroup>) -> Self {
        let model = ObservatoryModel::handle(ctx);
        let observatory_view: ViewHandle<ObservatoryPanelView> =
            ctx.add_typed_action_view(|ctx| ObservatoryPanelView::new(model.clone(), ctx));
        // 打开为独立 tab 即面板 open（gate 5s 轮询）+ 立即刷新
        ObservatoryModel::handle(ctx).update(ctx, |m, ctx| {
            m.set_panel_open(true, ctx);
            m.refresh(ctx);
        });
        Self::from_view(observatory_view, ctx)
    }

    fn from_view(
        observatory_view: ViewHandle<ObservatoryPanelView>,
        ctx: &mut AppContext,
    ) -> Self {
        let pane_configuration =
            ctx.add_model(|_| PaneConfiguration::new(crate::t!("observatory-pane-title")));
        let view = ctx.add_typed_action_view(observatory_view.window_id(ctx), |ctx| {
            let pane_id = PaneId::from_observatory_pane_ctx(ctx);
            PaneView::new(pane_id, observatory_view, (), pane_configuration.clone(), ctx)
        });

        Self {
            view,
            pane_configuration,
        }
    }

    fn observatory_view(&self, ctx: &AppContext) -> ViewHandle<ObservatoryPanelView> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for ObservatoryPane {
    fn id(&self) -> PaneId {
        PaneId::from_observatory_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let pane_id = self.id();
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        // 双订阅（settings_pane 同款）：child view 收 PaneEvent::Close（X 关闭），
        // PaneView 收拖拽/放下事件。
        let child = self.observatory_view(ctx);
        ctx.subscribe_to_view(&child, move |pane_group, _, event, ctx| {
            pane_group.handle_pane_event(pane_id, event, ctx);
        });
        ctx.subscribe_to_view(&self.view, move |group, _, event, ctx| {
            group.handle_pane_view_event(pane_id, event, ctx);
        });

        // attach 即面板 open（轮询恢复）：覆盖 UndoClosedPanes 关闭→恢复路径
        // （detach 置 false 后重挂需重开），幂等：初次挂载时 new() 已置 true。
        ObservatoryModel::handle(ctx).update(ctx, |m, ctx| {
            m.set_panel_open(true, ctx);
        });
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let child = self.observatory_view(ctx);
        ctx.unsubscribe_to_view(&child);
        ctx.unsubscribe_to_view(&self.view);
        // tab 关闭 → 面板视为关闭（停轮询）
        ObservatoryModel::handle(ctx).update(ctx, |m, ctx| {
            m.set_panel_open(false, ctx);
        });
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::Observatory
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.observatory_view(ctx)
            .update(ctx, BackingView::focus_contents)
    }

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        Ok(ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}

impl BackingView for ObservatoryPanelView {
    type PaneHeaderOverflowMenuAction = ();
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        _action: &Self::PaneHeaderOverflowMenuAction,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        use crate::pane_group::PaneEvent;
        ctx.emit(PaneEvent::Close);
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus_search(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &super::view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> super::view::HeaderContent {
        super::view::HeaderContent::simple(crate::t!("observatory-pane-title"))
    }

    fn set_focus_handle(
        &mut self,
        _focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        _ctx: &mut ViewContext<Self>,
    ) {
    }
}
