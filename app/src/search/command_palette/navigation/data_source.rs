use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::navigation::search::{
    FullTextSessionSearcher, MatchedSession, SessionMatchResult, SessionSearcher,
};
use crate::search::command_palette::navigation::search_item::SearchItem;
use crate::search::data_source::{DataSourceSearchError, Query, QueryResult};
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::search::SyncDataSource;
use crate::session_management::{SessionNavigationData, SessionSource};
use crate::workspace::PaneViewLocator;
use warpui::{AppContext, Entity, ModelHandle};

/// Data source that produces possible running sessions a user could navigate to.
pub struct DataSource {
    searcher: Box<dyn SessionSearcher>,
}

impl DataSource {
    pub fn new(active_session_handle: ModelHandle<SessionSource>) -> Self {
        // wasm 下 tantivy 不支持: full_text_searcher mod 仅在非 wasm 编译。
        // Dais 为桌面专属目标, wasm 构建路径不存在。
        let searcher = Box::new(FullTextSessionSearcher::new(active_session_handle));
        Self { searcher }
    }
}

impl SyncDataSource for DataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        self.searcher
            .search(&query.text.trim().to_lowercase(), app)
            .map_err(|err| {
                let search_error = DataSourceSearchError {
                    message: err.to_string(),
                };
                Box::new(search_error) as DataSourceRunErrorWrapper
            })
    }
}

impl DataSource {
    /// Returns a [`QueryResult`] for a session identified by `pane_view_locator`. `None` if no
    /// result was found with the given ID.
    pub fn query_result(
        &self,
        pane_view_locator: PaneViewLocator,
        app: &AppContext,
    ) -> Option<QueryResult<CommandPaletteItemAction>> {
        let session = SessionNavigationData::all_sessions(app)
            .find(|session| session.pane_view_locator() == pane_view_locator)?;

        let matched_session = MatchedSession {
            session,
            match_result: SessionMatchResult::no_match(),
        };

        let active_session_id = self.searcher.active_session_id(app);

        Some(SearchItem::new(matched_session, active_session_id).into())
    }
}

impl Entity for DataSource {
    type Event = ();
}
