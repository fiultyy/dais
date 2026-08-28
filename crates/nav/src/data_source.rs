//! NavDataSource — vertical_tabs 面板对宿主 (Workspace) 的只读数据契约。
//!
//! 当前 app 侧 `vertical_tabs.rs` 直接消费 `&Workspace` 具体类型 (7370 行,
//! 约 18 处签名引用)。v2 完全体把该文件整体迁入 nav crate 时,因 nav 禁止
//! 依赖 app (warp),必须经本 trait 解耦:vertical_tabs 逐函数把
//! `&Workspace` 参数改为 `&impl NavDataSource` (或泛型约束),函数体不变。
//!
//! 方法签名只出 POD/引用,不泄漏任何 app 侧 handle 类型
//! (如 `ViewHandle<EditorView>` — rename/search 编辑器一律以
//! `bool` / `Option<()>` 表达存在性)。
//!
//! v1 (本轮) 只定义契约 + 提供默认空实现,不接线;接线与文件本体迁移
//! 推迟到下一轮 commit。

use std::path::{Path, PathBuf};

use warpui::WindowId;

/// 单个 tab 的只读投影。`usize` 索引与宿主 `tabs` 顺序一致,由
/// [`NavDataSource`] 按索引取投影,不暴露 `TabData` 本体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavTabInfo {
    /// Tab 级自定义标题 (rename-tab 流程写入);`None` = 未设置。
    pub custom_title: Option<String>,
    /// 所属项目路径;`None` = 无项目 (始终可见)。
    pub project_path: Option<String>,
    /// 该 tab 是否被用户手动拖拽中。
    pub is_dragging: bool,
}

/// 项目 rail 选中态下,某项目卡片的聚合信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavProjectInfo {
    /// 项目路径 (与 `active_project` 同一规范形)。
    pub path: String,
    /// 该项目名下的 tab 数。
    pub tab_count: usize,
}

/// tab 栏悬停位置 — 宿主 `TabBarHoverIndex` 的 POD 等价镜像。
/// v2 迁移后 nav 内部统一用本类型,避免依赖 pane_group 具体枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTabHover {
    /// 悬停在 tab `usize` 之前的插入带。
    BeforeTab(usize),
    /// 悬停在 tab `usize` 本体上。
    OverTab(usize),
}

/// vertical_tabs 面板对宿主状态的只读查询面。
///
/// 实现方 = app 侧 `Workspace`;消费方 = nav 侧 vertical_tabs 渲染逻辑。
/// 所有方法要求 `&self` 只读、无副作用,可在渲染帧内任意次调用。
pub trait NavDataSource {
    // ---- tabs 快照迭代 (只读投影) ----

    /// tab 总数 (含被项目过滤器隐藏的 tab)。
    fn tab_count(&self) -> usize;

    /// 按 flat 索引取 tab 投影;越界返回 `None`。
    fn tab_info(&self, index: usize) -> Option<NavTabInfo>;

    /// 按项目路径过滤的 tab 索引列表 (保序)。
    fn tab_indices_with_project(&self, project: &Path) -> Vec<usize>;

    /// 无项目归属的 tab 索引列表 (保序) — "未分组"卡片数据源。
    fn tab_indices_without_project(&self, known_projects: &[PathBuf]) -> Vec<usize>;

    /// 当前项目过滤器下可见的 tab flat 索引 (保序)。
    /// 未选中项目 = 全部索引。
    fn visible_tab_indices(&self) -> Vec<usize>;

    // ---- 焦点 / 悬停 ----

    /// 当前激活 tab 的 flat 索引。
    fn active_tab_index(&self) -> usize;

    /// 当前激活 tab 所属项目;`None` = 未选中项目 (rail 显示全部)。
    fn active_project(&self) -> Option<&Path>;

    /// tab 栏悬停位置;`None` = 未悬停。
    fn hovered_tab_index(&self) -> Option<NavTabHover>;

    // ---- 窗口 / 面板状态 ----

    /// 宿主窗口 id (元素定位、tooltip 钳制等按窗口查询用)。
    fn window_id(&self) -> WindowId;

    /// 当前工作区状态快照 (改名/弹窗等互斥 UI 状态)。
    fn current_workspace_state(&self) -> WorkspaceStateSnapshot;

    /// 新会话下拉菜单是否打开。
    fn show_new_session_dropdown_menu(&self) -> bool;

    /// 某 flat 索引 tab 的右键菜单是否打开。
    fn is_tab_context_menu_open(&self, tab_index: usize) -> bool;

    // ---- 编辑器存在性 (以 bool/Option<()> 表达,不泄漏 handle) ----

    /// tab 改名编辑器是否处于活跃 (打开) 状态。
    fn tab_rename_editor_active(&self) -> bool;

    /// pane 改名编辑器是否处于活跃 (打开) 状态。
    fn pane_rename_editor_active(&self) -> bool;
}

impl NavDataSource for () {
    fn tab_count(&self) -> usize {
        0
    }

    fn tab_info(&self, _index: usize) -> Option<NavTabInfo> {
        None
    }

    fn tab_indices_with_project(&self, _project: &Path) -> Vec<usize> {
        Vec::new()
    }

    fn tab_indices_without_project(&self, _known_projects: &[PathBuf]) -> Vec<usize> {
        Vec::new()
    }

    fn visible_tab_indices(&self) -> Vec<usize> {
        Vec::new()
    }

    fn active_tab_index(&self) -> usize {
        0
    }

    fn active_project(&self) -> Option<&Path> {
        None
    }

    fn hovered_tab_index(&self) -> Option<NavTabHover> {
        None
    }

    fn window_id(&self) -> WindowId {
        WindowId::from_usize(usize::MAX)
    }

    fn current_workspace_state(&self) -> WorkspaceStateSnapshot {
        WorkspaceStateSnapshot::default()
    }

    fn show_new_session_dropdown_menu(&self) -> bool {
        false
    }

    fn is_tab_context_menu_open(&self, _tab_index: usize) -> bool {
        false
    }

    fn tab_rename_editor_active(&self) -> bool {
        false
    }

    fn pane_rename_editor_active(&self) -> bool {
        false
    }
}

/// [`NavDataSource::current_workspace_state`] 返回的互斥 UI 状态快照。
/// 只投影 vertical_tabs 实际读取的字段,v2 迁移后按需扩充。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceStateSnapshot {
    /// agent 管理视图是否打开 (激活 tab 判定参与项)。
    pub is_agent_management_view_open: bool,
    /// 是否有 tab 正在改名 (任意索引)。
    pub is_tab_being_renamed: bool,
    /// 正在改名的 tab 索引。
    pub tab_being_renamed: Option<usize>,
    /// 是否有 pane 正在改名。
    pub is_any_pane_being_renamed: bool,
}
