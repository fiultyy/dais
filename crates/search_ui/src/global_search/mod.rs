//! P-ARC R8-B: 全局搜索配置/事件/模型(原 workspace/view/global_search)。

pub struct SearchConfig {
    pub use_regex: bool,
    pub use_case_sensitivity: bool,
}

/// 搜索进度事件(原 view.rs 内定义,纯数据,随模型迁移)。
pub enum GlobalSearchEvent {
    Started {
        search_id: u32,
    },
    Progress {
        search_id: u32,
        result: warp_ripgrep::search::Match,
    },
    ProgressBatch {
        search_id: u32,
        items: Vec<warp_ripgrep::search::Match>,
    },
    Completed {
        search_id: u32,
        total_match_count: usize,
    },
    Failed {
        search_id: u32,
        error: String,
    },
}

#[cfg_attr(not(target_family = "wasm"), path = "model.rs")]
#[cfg_attr(target_family = "wasm", path = "model_wasm.rs")]
pub mod model;
