use super::env_var_collection_search_item::EnvVarCollectionSearchItem;
use super::notebook_search_item::NotebookSearchItem;
use crate::cloud_object::model::persistence::{ObjectStoreEvent, ObjectStoreModel};
use crate::cloud_object::{
    GenericStringObjectFormat, JsonObjectType, ObjectType, StoredObject, StoredObjectLocation,
};
use crate::drive::folders::FolderObject;
use crate::env_vars::EnvVarCollectionObject;
use crate::notebooks::NotebookObject;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{DataSourceSearchError, Query, QueryResult};
use crate::search::env_var_collections::fuzzy_match::FuzzyMatchEnvVarCollectionResult;
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::search::notebooks::fuzzy_match::FuzzyMatchNotebookResult;
use crate::search::QueryFilter;
use crate::server::ids::{ObjectUid, SyncId};
use std::collections::HashMap;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

/// Datasource that searches against all Dais Drive objects
pub struct DataSource {
    searcher: Box<dyn WarpDriveSearcher>,
}

impl DataSource {
    #[cfg(not(target_family = "wasm"))]
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        if warp_core::features::FeatureFlag::UseTantivySearch.is_enabled() {
            // 编译期 gate: feature 缺省时 full_text 分支不存在, 恒走 fuzzy
            #[cfg(feature = "use_tantivy_search")]
            {
                return Self::new_full_text(ctx);
            }
            Self::new_fuzzy(ctx)
        } else {
            Self::new_fuzzy(ctx)
        }
    }

    #[cfg(target_family = "wasm")]
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self::new_fuzzy(ctx)
    }

    pub fn new_fuzzy(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(
            &ObjectStoreModel::handle(ctx),
            Self::handle_cloud_object_updated,
        );
        let mut searcher = Box::new(FuzzyWarpDriveSearcher::default());
        searcher.refresh_search_index(ctx).unwrap_or_else(|err| {
            log::error!("Error refreshing search index: {err:?}");
        });
        DataSource { searcher }
    }

    // gate 与 full_text_searcher mod 一致: feature 缺省时本函数与 mod 一同不存在
    #[cfg(all(not(target_family = "wasm"), feature = "use_tantivy_search"))]
    fn new_full_text(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(
            &ObjectStoreModel::handle(ctx),
            Self::handle_cloud_object_updated,
        );
        let mut searcher = Box::new(full_text_searcher::FullTextWarpDriveSearcher::new(
            ctx.background_executor(),
        ));
        searcher.refresh_search_index(ctx).unwrap_or_else(|err| {
            log::error!("Error refreshing search index: {err:?}");
        });
        DataSource { searcher }
    }

    fn handle_cloud_object_updated(
        &mut self,
        event: &ObjectStoreEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        // When the initial bulk load completes, rebuild the entire search index once.
        // Per-object events are suppressed at the source during initial load, so this
        // is the only event we receive from that batch.
        if let ObjectStoreEvent::InitialLoadCompleted = event {
            self.searcher
                .refresh_search_index(ctx)
                .unwrap_or_else(|err| {
                    log::error!("Error refreshing search index after initial load: {err:?}");
                });
            return;
        }

        match event {
            ObjectStoreEvent::ObjectCreated { type_and_id }
            | ObjectStoreEvent::ObjectUntrashed { type_and_id, .. }
            | ObjectStoreEvent::ObjectMoved { type_and_id, .. }
            | ObjectStoreEvent::ObjectUpdated { type_and_id, .. } => {
                if let Some(obj) = ObjectStoreModel::as_ref(ctx).get_by_uid(&type_and_id.uid()) {
                    // Insertion will overwrite the object if it already exists.
                    self.searcher
                        .insert_searchable_object(obj, type_and_id.object_type(), ctx)
                        .unwrap_or_else(|err| {
                            log::error!("Error inserting object into search index: {err:?}");
                        });
                } else {
                    log::error!("Object with ID {type_and_id:?} not found in ObjectStoreModel");
                }
            }
            ObjectStoreEvent::ObjectTrashed { type_and_id, .. } => self
                .searcher
                .delete_searchable_object(type_and_id.uid(), type_and_id.object_type(), ctx)
                .unwrap_or_else(|err| {
                    log::error!("Error deleting object from search index: {err:?}");
                }),
            _ => {}
        }
    }
}

impl crate::search::mixer::SyncDataSource for DataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let mut filtered_cloud_objects = Vec::new();

        let should_include_all_drive_objects = Self::include_all_drive_objects_in_result(query);

        if query.filters.contains(&QueryFilter::Notebooks) || should_include_all_drive_objects {
            filtered_cloud_objects.extend(
                self.searcher
                    .search_notebook(&query.text.to_lowercase(), app)
                    .map_err(|err| {
                        Box::new(DataSourceSearchError {
                            message: err.to_string(),
                        }) as DataSourceRunErrorWrapper
                    })?
                    .into_iter()
                    .map(QueryResult::from),
            );
        }

        if query.filters.contains(&QueryFilter::Plans) || should_include_all_drive_objects {
            filtered_cloud_objects.extend(
                self.searcher
                    .search_plans(&query.text.to_lowercase(), app)
                    .map_err(|err| {
                        Box::new(DataSourceSearchError {
                            message: err.to_string(),
                        }) as DataSourceRunErrorWrapper
                    })?
                    .into_iter()
                    .map(QueryResult::from),
            );
        }

        if query.filters.contains(&QueryFilter::EnvironmentVariables)
            || should_include_all_drive_objects
        {
            filtered_cloud_objects.extend(
                self.searcher
                    .search_env_var(&query.text.to_lowercase(), app)
                    .map_err(|err| {
                        Box::new(DataSourceSearchError {
                            message: err.to_string(),
                        }) as DataSourceRunErrorWrapper
                    })?
                    .into_iter()
                    .map(QueryResult::from),
            );
        }

        Ok(filtered_cloud_objects)
    }
}

impl DataSource {
    /// If we are using the drive filter, or there are no filters,
    /// then we want to include all searchable drive objects
    fn include_all_drive_objects_in_result(query: &Query) -> bool {
        query.filters.contains(&QueryFilter::Drive) || query.filters.is_empty()
    }
    /// Returns a [`QueryResult`] for a notebook identified by `sync_id`. `None` if no result was
    /// found with the given ID.
    pub fn query_result(
        &self,
        sync_id: &SyncId,
        app: &AppContext,
    ) -> Option<QueryResult<CommandPaletteItemAction>> {
        let object = ObjectStoreModel::as_ref(app).get_by_uid(&sync_id.uid())?;
        let notebook: Option<&NotebookObject> = object.into();
        if let Some(notebook) = notebook {
            return Some(QueryResult::from(NotebookSearchItem {
                match_result: FuzzyMatchNotebookResult::no_match(),
                notebook: notebook.clone(),
            }));
        }

        let env_var_collection: Option<&EnvVarCollectionObject> = object.into();
        if let Some(env_var_collection) = env_var_collection {
            return Some(QueryResult::from(EnvVarCollectionSearchItem {
                match_result: FuzzyMatchEnvVarCollectionResult::no_match(),
                cloud_env_var_collection: env_var_collection.clone(),
            }));
        }

        None
    }
}

impl Entity for DataSource {
    type Event = ();
}

trait WarpDriveSearcher {
    fn insert_searchable_object(
        &mut self,
        object: &dyn StoredObject,
        object_type: ObjectType,
        app: &AppContext,
    ) -> anyhow::Result<()>;

    fn delete_searchable_object(
        &mut self,
        uid: ObjectUid,
        object_type: ObjectType,
        app: &AppContext,
    ) -> anyhow::Result<()>;

    /// Clear and rebuild the search index.
    fn refresh_search_index(&mut self, app: &AppContext) -> anyhow::Result<()>;

    fn search_notebook(
        &self,
        query: &str,
        app: &AppContext,
    ) -> anyhow::Result<Vec<NotebookSearchItem>>;

    fn search_env_var(
        &self,
        query: &str,
        app: &AppContext,
    ) -> anyhow::Result<Vec<EnvVarCollectionSearchItem>>;

    fn search_plans(
        &self,
        query: &str,
        app: &AppContext,
    ) -> anyhow::Result<Vec<NotebookSearchItem>>;
}

#[derive(Default)]
struct FuzzyWarpDriveSearcher {
    notebooks: HashMap<ObjectUid, NotebookObject>,
    env_vars: HashMap<ObjectUid, EnvVarCollectionObject>,
}

impl WarpDriveSearcher for FuzzyWarpDriveSearcher {
    fn insert_searchable_object(
        &mut self,
        object: &dyn StoredObject,
        object_type: ObjectType,
        app: &AppContext,
    ) -> anyhow::Result<()> {
        match object_type {
            ObjectType::Notebook => {
                let notebook: Option<&NotebookObject> = object.into();
                if let Some(notebook) = notebook {
                    self.notebooks.insert(notebook.uid(), notebook.clone());
                } else {
                    anyhow::bail!("Expected NotebookObject, got {:?}", object);
                }
            }
            ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                JsonObjectType::EnvVarCollection,
            )) => {
                let env_var: Option<&EnvVarCollectionObject> = object.into();
                if let Some(env_var) = env_var {
                    self.env_vars.insert(env_var.uid(), env_var.clone());
                } else {
                    anyhow::bail!("Expected EnvVarCollectionObject, got {:?}", object);
                }
            }
            ObjectType::Folder => {
                let folder: Option<&FolderObject> = object.into();
                if let Some(folder) = folder {
                    let location = StoredObjectLocation::Folder(folder.id);
                    for obj in ObjectStoreModel::as_ref(app)
                        .active_cloud_objects_in_location_without_descendents(location, app)
                    {
                        self.insert_searchable_object(obj, obj.object_type(), app)?
                    }
                } else {
                    anyhow::bail!("Expected FolderObject, got {:?}", object);
                }
            }
            // We don't care about other object types for now.
            _ => {}
        }
        Ok(())
    }

    fn delete_searchable_object(
        &mut self,
        uid: ObjectUid,
        object_type: ObjectType,
        app: &AppContext,
    ) -> anyhow::Result<()> {
        match object_type {
            ObjectType::Notebook => {
                self.notebooks.remove(&uid);
            }
            ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                JsonObjectType::EnvVarCollection,
            )) => {
                self.env_vars.remove(&uid);
            }
            ObjectType::Folder => {
                let model = ObjectStoreModel::as_ref(app);
                let Some(obj) = model.get_by_uid(&uid) else {
                    anyhow::bail!("Object with ID {:?} not found in ObjectStoreModel", uid);
                };
                let folder: Option<&FolderObject> = obj.into();
                if let Some(folder) = folder {
                    let location = StoredObjectLocation::Folder(folder.id);
                    for obj in
                        model.trashed_cloud_objects_in_location_without_descendents(location, app)
                    {
                        self.delete_searchable_object(obj.uid(), obj.object_type(), app)?
                    }
                } else {
                    anyhow::bail!("Expected FolderObject, got {:?}", obj);
                }
            }
            // We don't care about other object types for now.
            _ => {}
        }
        Ok(())
    }

    fn refresh_search_index(&mut self, app: &AppContext) -> anyhow::Result<()> {
        self.notebooks.clear();
        self.env_vars.clear();
        let model = ObjectStoreModel::as_ref(app);
        // Single pass with memoized is_trashed: O(N) instead of O(3×N×D).
        let active_uids = model.active_object_uids();
        for object in model.cloud_objects() {
            if !active_uids.contains(&object.uid()) {
                continue;
            }
            if let Some(notebook) = <Option<&NotebookObject>>::from(object.as_ref()) {
                self.notebooks.insert(notebook.uid(), notebook.clone());
            } else if let Some(env_var) = <Option<&EnvVarCollectionObject>>::from(object.as_ref()) {
                self.env_vars.insert(env_var.uid(), env_var.clone());
            }
        }
        Ok(())
    }

    fn search_notebook(
        &self,
        query: &str,
        app: &AppContext,
    ) -> anyhow::Result<Vec<NotebookSearchItem>> {
        let cloud_notebooks = ObjectStoreModel::as_ref(app).get_all_active_notebooks();
        Ok(cloud_notebooks
            .filter_map(|notebook| {
                FuzzyMatchNotebookResult::try_match(query, notebook, app).map(|match_result| {
                    NotebookSearchItem {
                        match_result,
                        notebook: notebook.clone(),
                    }
                })
            })
            .collect())
    }

    fn search_plans(
        &self,
        query: &str,
        app: &AppContext,
    ) -> anyhow::Result<Vec<NotebookSearchItem>> {
        let cloud_notebooks = ObjectStoreModel::as_ref(app)
            .get_all_active_notebooks()
            .filter(|notebook| notebook.model().ai_document_id.is_some());
        Ok(cloud_notebooks
            .filter_map(|notebook| {
                FuzzyMatchNotebookResult::try_match(query, notebook, app).map(|match_result| {
                    NotebookSearchItem {
                        match_result,
                        notebook: notebook.clone(),
                    }
                })
            })
            .collect())
    }

    fn search_env_var(
        &self,
        query: &str,
        app: &AppContext,
    ) -> anyhow::Result<Vec<EnvVarCollectionSearchItem>> {
        let cloud_env_var_collections =
            ObjectStoreModel::as_ref(app).get_all_active_env_var_collections();

        Ok(cloud_env_var_collections
            .filter_map(|cloud_env_var_collection| {
                FuzzyMatchEnvVarCollectionResult::try_match(
                    query,
                    &cloud_env_var_collection.model().string_model,
                    cloud_env_var_collection.breadcrumbs(app).as_str(),
                )
                .map(|match_result| EnvVarCollectionSearchItem {
                    match_result,
                    cloud_env_var_collection: cloud_env_var_collection.clone(),
                })
            })
            .collect())
    }
}

// tantivy 为 optional dep: 该全文搜索 mod 仅在 use_tantivy_search feature 下编译
#[cfg(all(not(target_family = "wasm"), feature = "use_tantivy_search"))]
mod full_text_searcher {
    use std::sync::Arc;

    use crate::cloud_object::model::persistence::ObjectStoreModel;
    use crate::cloud_object::{
        GenericStringObjectFormat, JsonObjectType, ObjectType, StoredObject, StoredObjectLocation,
    };
    use crate::define_search_schema;
    use crate::drive::folders::FolderObject;
    use crate::env_vars::EnvVarCollectionObject;
    use crate::notebooks::manager::NotebookManager;
    use crate::notebooks::NotebookObject;
    use crate::search::command_palette::warp_drive::data_source::WarpDriveSearcher;
    use crate::search::command_palette::warp_drive::env_var_collection_search_item::{
        EnvVarCollectionSearchItem, ENV_VAR_NAME_SEPARATOR,
    };
    use crate::search::command_palette::warp_drive::notebook_search_item::NotebookSearchItem;
    use crate::search::env_var_collections::fuzzy_match::FuzzyMatchEnvVarCollectionResult;
    use crate::search::notebooks::fuzzy_match::FuzzyMatchNotebookResult;
    use crate::search::searcher::{AsyncSearcher, DEFAULT_MEMORY_BUDGET, SCORE_CONVERSION_FACTOR};
    use crate::server::ids::ObjectUid;
    use fuzzy_match::FuzzyMatchResult;
    use itertools::Itertools;
    use warpui::r#async::executor::Background;
    use warpui::{AppContext, SingletonEntity};

    /// Memory budget for the search index of dais drive.
    /// Dais could potentially have a lot of objects, so we increase it from the default of 50MB to 100MB
    const MEMORY_BUDGET: usize = 100_000_000; // TODO: is 100MB really necessary?

    // All Dais Drive objects are boosted due to multiple fields being a part of the same total score,
    // putting them at an inherent disadvantage, as each field would only have a fractional weight.
    define_search_schema!(
        schema_name: NOTEBOOK_SEARCH_SCHEMA,
        config_name: NotebookConfig,
        search_doc: NotebookSearchDocument,
        identifying_doc: NotebookIdDocument,
        search_fields: [
            name: 0.6,
            content: 0.2,
            folder: 0.2
        ],
        id_fields: [
            uid: String
        ],
        boost_factor: 1.15 // Boosted by only 1.15 as the name field has a weighting of 0.6 instead of 0.5 like others.
    );
    define_search_schema!(
        schema_name: ENVVAR_SEARCH_SCHEMA,
        config_name: EnvVarConfig,
        search_doc: EnvVarSearchDocument,
        identifying_doc: EnvVarIdDocument,
        search_fields: [
            title: 0.5,
            var_name: 0.3,
            description: 0.1,
            folder: 0.1
        ],
        id_fields: [
            uid: String
        ],
        boost_factor: 1.3
    );

    pub(crate) struct FullTextWarpDriveSearcher {
        notebook_searcher: AsyncSearcher<NotebookConfig>,
        env_var_searcher: AsyncSearcher<EnvVarConfig>,
    }

    impl FullTextWarpDriveSearcher {
        fn search_notebooks_with_filter(
            &self,
            query: &str,
            filter_by_plan: bool,
            app: &AppContext,
        ) -> anyhow::Result<Vec<NotebookSearchItem>> {
            if query.is_empty() {
                return Ok(self
                    .notebook_searcher
                    .get_all_doc_ids()?
                    .into_iter()
                    .filter_map(|search_match| {
                        let notebook: Option<&NotebookObject> = ObjectStoreModel::as_ref(app)
                            .get_by_uid(&search_match.uid)?
                            .into();
                        let notebook = notebook?;
                        if filter_by_plan && notebook.model().ai_document_id.is_none() {
                            return None;
                        }

                        Some(NotebookSearchItem {
                            match_result: FuzzyMatchNotebookResult::no_match(),
                            notebook: notebook.clone(),
                        })
                    })
                    .collect());
            }

            Ok(self
                .notebook_searcher
                .search_id(query)?
                .into_iter()
                .filter_map(|search_match| {
                    let notebook: Option<&NotebookObject> = ObjectStoreModel::as_ref(app)
                        .get_by_uid(&search_match.values.uid)?
                        .into();
                    let notebook = notebook?;

                    if filter_by_plan && notebook.model().ai_document_id.is_none() {
                        return None;
                    }

                    // Since Tantivy only produces a single score for the entire document, we put it as the score of all 3.
                    let name_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.name,
                    });
                    let content_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.content,
                    });
                    let folder_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.folder,
                    });

                    Some(NotebookSearchItem {
                        match_result: FuzzyMatchNotebookResult {
                            name_match_result,
                            content_match_result,
                            folder_match_result,
                        },
                        notebook: notebook.clone(),
                    })
                })
                .collect())
        }
    }

    impl WarpDriveSearcher for FullTextWarpDriveSearcher {
        fn insert_searchable_object(
            &mut self,
            object: &dyn StoredObject,
            object_type: ObjectType,
            app: &AppContext,
        ) -> anyhow::Result<()> {
            match object_type {
                ObjectType::Notebook => {
                    let notebook: Option<&NotebookObject> = object.into();
                    if let Some(notebook) = notebook {
                        let name = notebook.model().title.to_lowercase();
                        let content = NotebookManager::as_ref(app)
                            .notebook_raw_text(notebook.id)
                            .unwrap_or(&notebook.model().data)
                            .to_lowercase();
                        let folder = notebook.breadcrumbs(app).to_lowercase();
                        let uid = notebook.uid();

                        let document = NotebookSearchDocument {
                            name,
                            content,
                            folder,
                            uid,
                        };
                        self.notebook_searcher.insert_document_async(document)
                    } else {
                        anyhow::bail!("Expected NotebookObject, got {:?}", object);
                    }
                }
                ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                    JsonObjectType::EnvVarCollection,
                )) => {
                    let env_var: Option<&EnvVarCollectionObject> = object.into();
                    if let Some(cloud_env_var) = env_var {
                        let env_var_collection = &cloud_env_var.model().string_model;

                        let title = env_var_collection
                            .title
                            .as_ref()
                            .unwrap_or(&"".to_owned())
                            .to_lowercase();
                        let var_name = env_var_collection
                            .vars
                            .iter()
                            .map(|var| &var.name)
                            .join(ENV_VAR_NAME_SEPARATOR)
                            .to_lowercase();
                        let description = env_var_collection
                            .description
                            .as_ref()
                            .unwrap_or(&"".to_owned())
                            .to_lowercase();
                        let folder = cloud_env_var.breadcrumbs(app).to_lowercase();

                        let document = EnvVarSearchDocument {
                            title,
                            var_name,
                            description,
                            folder,
                            uid: cloud_env_var.uid(),
                        };
                        self.env_var_searcher.insert_document_async(document)
                    } else {
                        anyhow::bail!("Expected EnvVarCollectionObject, got {:?}", object);
                    }
                }
                ObjectType::Folder => {
                    let folder: Option<&FolderObject> = object.into();
                    if let Some(folder) = folder {
                        let location = StoredObjectLocation::Folder(folder.id);
                        for obj in ObjectStoreModel::as_ref(app)
                            .active_cloud_objects_in_location_without_descendents(location, app)
                        {
                            self.insert_searchable_object(obj, obj.object_type(), app)?
                        }
                        Ok(())
                    } else {
                        anyhow::bail!("Expected FolderObject, got {:?}", object);
                    }
                }
                // We don't care about other object types for now.
                _ => Ok(()),
            }
        }

        fn delete_searchable_object(
            &mut self,
            uid: ObjectUid,
            object_type: ObjectType,
            app: &AppContext,
        ) -> anyhow::Result<()> {
            match object_type {
                ObjectType::Notebook => {
                    let identifying_entry = NotebookIdDocument { uid };
                    self.notebook_searcher
                        .delete_document_async(identifying_entry)
                }
                ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                    JsonObjectType::EnvVarCollection,
                )) => {
                    let identifying_entry = EnvVarIdDocument { uid };
                    self.env_var_searcher
                        .delete_document_async(identifying_entry)
                }
                ObjectType::Folder => {
                    let Some(obj) = ObjectStoreModel::as_ref(app).get_by_uid(&uid) else {
                        anyhow::bail!("Object with ID {:?} not found in ObjectStoreModel", uid);
                    };
                    let folder: Option<&FolderObject> = obj.into();
                    if let Some(folder) = folder {
                        let location = StoredObjectLocation::Folder(folder.id);
                        for obj in ObjectStoreModel::as_ref(app)
                            .trashed_cloud_objects_in_location_without_descendents(location, app)
                        {
                            self.delete_searchable_object(obj.uid(), obj.object_type(), app)?
                        }
                        Ok(())
                    } else {
                        anyhow::bail!("Expected FolderObject, got {:?}", folder);
                    }
                }
                // We don't care about other object types for now.
                _ => Ok(()),
            }
        }

        fn refresh_search_index(&mut self, app: &AppContext) -> anyhow::Result<()> {
            let model = ObjectStoreModel::as_ref(app);
            // Pre-compute active UIDs in a single O(N) pass with memoized is_trashed,
            // instead of 3 separate O(N×D) passes.
            let active_uids = model.active_object_uids();

            self.notebook_searcher.clear_search_index_async()?;
            let notebook_docs = model
                .cloud_objects()
                .filter(|obj| active_uids.contains(&obj.uid()))
                .filter_map(|obj| {
                    let notebook: Option<&NotebookObject> = obj.as_ref().into();
                    notebook.map(|notebook| {
                        let name = notebook.model().title.to_lowercase();
                        let content = NotebookManager::as_ref(app)
                            .notebook_raw_text(notebook.id)
                            .unwrap_or(&notebook.model().data)
                            .to_lowercase();
                        let folder = notebook.breadcrumbs(app).to_lowercase();
                        let uid = notebook.uid();
                        NotebookSearchDocument {
                            name,
                            content,
                            folder,
                            uid,
                        }
                    })
                });
            self.notebook_searcher.build_index_async(notebook_docs)?;

            self.env_var_searcher.clear_search_index_async()?;
            let env_var_docs = model
                .cloud_objects()
                .filter(|obj| active_uids.contains(&obj.uid()))
                .filter_map(|obj| {
                    let cloud_env_var: Option<&EnvVarCollectionObject> = obj.as_ref().into();
                    cloud_env_var.map(|cloud_env_var| {
                        let env_var_collection = &cloud_env_var.model().string_model;
                        let title = env_var_collection
                            .title
                            .as_ref()
                            .unwrap_or(&"".to_owned())
                            .to_lowercase();
                        let var_name = env_var_collection
                            .vars
                            .iter()
                            .map(|var| &var.name)
                            .join(ENV_VAR_NAME_SEPARATOR)
                            .to_lowercase();
                        let description = env_var_collection
                            .description
                            .as_ref()
                            .unwrap_or(&"".to_owned())
                            .to_lowercase();
                        let folder = cloud_env_var.breadcrumbs(app).to_lowercase();
                        EnvVarSearchDocument {
                            title,
                            var_name,
                            description,
                            folder,
                            uid: cloud_env_var.uid(),
                        }
                    })
                });
            self.env_var_searcher.build_index_async(env_var_docs)?;

            Ok(())
        }

        fn search_notebook(
            &self,
            query: &str,
            app: &AppContext,
        ) -> anyhow::Result<Vec<NotebookSearchItem>> {
            self.search_notebooks_with_filter(query, false, app)
        }

        fn search_plans(
            &self,
            query: &str,
            app: &AppContext,
        ) -> anyhow::Result<Vec<NotebookSearchItem>> {
            self.search_notebooks_with_filter(query, true, app)
        }

        fn search_env_var(
            &self,
            query: &str,
            app: &AppContext,
        ) -> anyhow::Result<Vec<EnvVarCollectionSearchItem>> {
            if query.is_empty() {
                return Ok(self
                    .env_var_searcher
                    .get_all_doc_ids()?
                    .into_iter()
                    .filter_map(|search_match| {
                        let env_var_collection: Option<&EnvVarCollectionObject> =
                            ObjectStoreModel::as_ref(app)
                                .get_by_uid(&search_match.uid)?
                                .into();
                        let env_var_collection = env_var_collection?;

                        Some(EnvVarCollectionSearchItem {
                            match_result: FuzzyMatchEnvVarCollectionResult::no_match(),
                            cloud_env_var_collection: env_var_collection.clone(),
                        })
                    })
                    .collect());
            }

            Ok(self
                .env_var_searcher
                .search_id(query)?
                .into_iter()
                .filter_map(|search_match| {
                    let env_var_collection: Option<&EnvVarCollectionObject> =
                        ObjectStoreModel::as_ref(app)
                            .get_by_uid(&search_match.values.uid)?
                            .into();
                    let env_var_collection = env_var_collection?;

                    // Since Tantivy only produces a single score for the entire document, we put it as the score of all 3.
                    let title_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.title,
                    });
                    let var_name_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.var_name,
                    });
                    let description_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.description,
                    });
                    let breadcrumbs_match_result = Some(FuzzyMatchResult {
                        score: (search_match.score * SCORE_CONVERSION_FACTOR) as i64,
                        matched_indices: search_match.highlights.folder,
                    });

                    Some(EnvVarCollectionSearchItem {
                        match_result: FuzzyMatchEnvVarCollectionResult {
                            title_match_result,
                            var_name_match_result,
                            description_match_result,
                            breadcrumbs_match_result,
                        },
                        cloud_env_var_collection: env_var_collection.clone(),
                    })
                })
                .collect())
        }
    }

    impl FullTextWarpDriveSearcher {
        pub(crate) fn new(background: Arc<Background>) -> Self {
            FullTextWarpDriveSearcher {
                notebook_searcher: NOTEBOOK_SEARCH_SCHEMA
                    .create_async_searcher(MEMORY_BUDGET, background.clone()),
                env_var_searcher: ENVVAR_SEARCH_SCHEMA
                    .create_async_searcher(DEFAULT_MEMORY_BUDGET, background),
            }
        }
    }
}
