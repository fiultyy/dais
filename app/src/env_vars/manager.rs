use crate::{
    objects::{model::persistence::ObjectStoreModel, Owner},
    env_vars::view::env_var_collection::EnvVarCollectionView,
    pane_group::{EnvVarCollectionPane, PaneContent},
    safe_warn,
    server::ids::SyncId,
    PaneViewLocator, WindowId,
};
use std::collections::{hash_map::Entry, HashMap};
use warpui::{Entity, EntityId, ModelContext, SingletonEntity, WeakViewHandle};

pub struct EnvVarCollectionManager {
    panes_by_hashed_id: HashMap<String, EnvVarCollectionPaneData>,
}

#[derive(Debug, Clone)]
pub enum EnvVarCollectionSource {
    Existing(SyncId),
    New {
        title: Option<String>,
        owner: Owner,
        initial_folder_id: Option<SyncId>,
    },
}

/// Manages EnvVarCollection panes
impl EnvVarCollectionManager {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        // Dais:client_id→server_id 转换事件仅云端创建成功后触发,
        // Dais 无云 = 永不触发。Phase 2c‑1 移除 ObjectStore 订阅与死处理函数。
        EnvVarCollectionManager {
            panes_by_hashed_id: HashMap::new(),
        }
    }

    /// If the collection is already open in a pane, finds the location of that pane.
    pub fn find_pane(
        &self,
        source: &EnvVarCollectionSource,
    ) -> Option<(WindowId, PaneViewLocator)> {
        match source {
            EnvVarCollectionSource::Existing(env_var_collection_id) => {
                let pane_data = self.panes_by_hashed_id.get(&env_var_collection_id.uid())?;
                Some((pane_data.window_id, pane_data.locator))
            }
            EnvVarCollectionSource::New { .. } => None,
        }
    }

    pub fn create_pane(
        &mut self,
        source: &EnvVarCollectionSource,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) -> EnvVarCollectionPane {
        let view = ctx.add_typed_action_view(window_id, EnvVarCollectionView::new);

        match source {
            EnvVarCollectionSource::Existing(env_var_collection_id) => {
                let env_var_collection = ObjectStoreModel::as_ref(ctx)
                    .get_env_var_collection(env_var_collection_id)
                    .cloned();
                if let Some(env_var_collection) = env_var_collection {
                    view.update(ctx, |view, ctx| view.load(env_var_collection, ctx));
                } else {
                    view.update(ctx, |view, ctx| {
                        view.wait_for_initial_load_then_load(
                            *env_var_collection_id,
                            window_id,
                            ctx,
                        );
                    });
                }
            }
            EnvVarCollectionSource::New {
                title: _,
                owner,
                initial_folder_id,
            } => view.update(ctx, |view, ctx| {
                view.open_new_env_var_collection(*owner, *initial_folder_id, ctx)
            }),
        }

        EnvVarCollectionPane::new(view, ctx)
    }

    pub fn register_pane(
        &mut self,
        pane: &EnvVarCollectionPane,
        pane_group_id: EntityId,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(env_var_collection_id) = pane
            .env_var_collection_view(ctx)
            .as_ref(ctx)
            .env_var_collection_id(ctx)
        else {
            log::warn!("EnvVarCollection pane has no ID");
            return;
        };

        let entry = self.panes_by_hashed_id.entry(env_var_collection_id.uid());
        if let Entry::Vacant(entry) = entry {
            entry.insert(EnvVarCollectionPaneData {
                env_var_collection_id,
                window_id,
                locator: PaneViewLocator {
                    pane_group_id,
                    pane_id: pane.id(),
                },
                handle: pane.env_var_collection_view(ctx).downgrade(),
            });
        } else {
            safe_warn!(
                safe: ("Ignoring duplicate EnvVarCollection pane registration"),
                full: ("Ignoring duplicate EnvVarCollection pane registration for {env_var_collection_id}")
            );
        }
    }

    pub fn deregister_pane(&mut self, pane: &EnvVarCollectionPane, ctx: &mut ModelContext<Self>) {
        let Some(env_var_collection_id) = pane
            .env_var_collection_view(ctx)
            .as_ref(ctx)
            .env_var_collection_id(ctx)
        else {
            log::warn!("EnvVarCollection pane has no ID");
            return;
        };

        // If an EVC pane is restored, the EVC may have been reopened in the meantime. In
        // that case, don't let closing the original pane clear out the new pane.
        if let Entry::Occupied(entry) = self.panes_by_hashed_id.entry(env_var_collection_id.uid()) {
            if entry.get().locator.pane_id == pane.id() {
                entry.remove();
            } else {
                log::warn!(
                    "Ignoring duplicate registration of panes for {}",
                    env_var_collection_id.uid()
                );
            }
        }
    }

    pub fn reload_collection(
        &mut self,
        source: &EnvVarCollectionSource,
        ctx: &mut ModelContext<Self>,
    ) {
        match source {
            EnvVarCollectionSource::Existing(env_var_collection_id) => {
                if let Some(pane_data) = self.panes_by_hashed_id.get(&env_var_collection_id.uid()) {
                    let env_var_collection = ObjectStoreModel::as_ref(ctx)
                        .get_env_var_collection(env_var_collection_id)
                        .cloned();
                    if let Some(env_var_collection) = env_var_collection {
                        if let Some(data) = pane_data.handle.upgrade(ctx) {
                            data.update(ctx, |view, ctx| view.load(env_var_collection, ctx));
                        }
                    }
                }
            }
            _ => log::warn!("Can only reload existing environment variable collection"),
        }
    }

    pub fn reset(&mut self) {
        self.panes_by_hashed_id.clear();
    }
}

struct EnvVarCollectionPaneData {
    env_var_collection_id: SyncId,
    window_id: WindowId,
    handle: WeakViewHandle<EnvVarCollectionView>,
    locator: PaneViewLocator,
}

impl Entity for EnvVarCollectionManager {
    type Event = ();
}

impl SingletonEntity for EnvVarCollectionManager {}


/// P-ARC R1: 宿主侧 EnvVarData 实现——桥接 crates/env_vars_ui 契约到 app 的
/// ObjectStoreViewModel/权限/匿名语义。
pub struct HostEnvVarData;

impl env_vars_ui::data::EnvVarData for HostEnvVarData {
    fn get_collection(&self, app: &warpui::AppContext, id: &objects::SyncId) -> Option<env_vars_ui::EnvVarCollectionObject> {
        use crate::objects::model::persistence::ObjectStoreModel;
        ObjectStoreModel::as_ref(app).get_env_var_collection(id).cloned()
    }
    fn initial_load_completed(&self, app: &warpui::AppContext) -> bool {
        use crate::objects::model::persistence::ObjectStoreModel;
        ObjectStoreModel::as_ref(app).initial_load_completed()
    }
    fn access_level(&self, app: &warpui::AppContext, uid: &str) -> env_vars_ui::data::AccessLevel {
        use crate::objects::model::view::ObjectStoreViewModel;
        use crate::drive::sharing::SharingAccessLevel;
        match ObjectStoreViewModel::as_ref(app).access_level(&uid.to_string(), app) {
            SharingAccessLevel::View => env_vars_ui::data::AccessLevel::View,
            SharingAccessLevel::Edit => env_vars_ui::data::AccessLevel::Edit,
            SharingAccessLevel::Full => env_vars_ui::data::AccessLevel::Full,
        }
    }
    fn editability(&self, app: &warpui::AppContext, uid: &str) -> env_vars_ui::data::Editability {
        use crate::objects::model::view::ObjectStoreViewModel;
        use crate::drive::sharing::ContentEditability;
        match ObjectStoreViewModel::as_ref(app).object_editability(&uid.to_string(), app) {
            ContentEditability::ReadOnly => env_vars_ui::data::Editability::ReadOnly,
            ContentEditability::RequiresLogin => env_vars_ui::data::Editability::RequiresLogin,
            ContentEditability::Editable => env_vars_ui::data::Editability::Editable,
        }
    }
    fn object_space(&self, app: &warpui::AppContext, uid: &str) -> Option<objects::Space> {
        use crate::objects::model::view::ObjectStoreViewModel;
        ObjectStoreViewModel::as_ref(app).object_space(&uid.to_string(), app)
    }
    fn is_trashed(&self, app: &warpui::AppContext, id: &objects::SyncId) -> Option<bool> {
        use crate::objects::model::persistence::ObjectStoreModel;
        ObjectStoreModel::as_ref(app).get_by_uid(&id.uid()).map(|o| o.metadata().trashed_ts.is_some())
    }
}

/// 全局共享宿主数据桥(无状态,直接构造即可)。
pub fn host_env_var_data() -> HostEnvVarData {
    HostEnvVarData
}
