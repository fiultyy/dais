use std::path::PathBuf;

use super::GlobalSearchEvent;
use super::SearchConfig;
use warpui::{Entity, ModelContext};

pub struct GlobalSearch {}

impl Entity for GlobalSearch {
    type Event = GlobalSearchEvent;
}

impl GlobalSearch {
    pub fn new() -> Self {
        GlobalSearch {}
    }

    pub fn abort_search(&mut self) {}

    pub fn run_search(
        &mut self,
        _pattern: String,
        _root: Vec<PathBuf>,
        _search_config: SearchConfig,
        _ctx: &mut ModelContext<Self>,
    ) {
    }
}

impl Default for GlobalSearch {
    fn default() -> Self {
        Self::new()
    }
}
