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

use pane_tree::{IPaneType, PaneId};
use serde::{Deserialize, Serialize};
use warpui::{AppContext, EntityId, WindowId};

/// app 侧 [`AnsiColorIdentifier`] 的 POD 枚举镜像。
/// 变体顺序与原类型一一对应;`as_u8`/`from_u8` 提供稳定 u8 视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NavAnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

impl NavAnsiColor {
    /// 稳定 u8 视图 (按变体声明顺序 0-7)。
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// 从 u8 视图还原;越界返回 `None`。
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Black,
            1 => Self::Red,
            2 => Self::Green,
            3 => Self::Yellow,
            4 => Self::Blue,
            5 => Self::Magenta,
            6 => Self::Cyan,
            7 => Self::White,
            _ => return None,
        })
    }
}

/// app 侧目录色配置项 [`DirectoryTabColor`] 的 POD 镜像。
/// `Suppressed` 条目不参与前缀匹配,但保留在快照中维持配置原貌。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavDirectoryColor {
    Suppressed,
    Unassigned,
    Color(NavAnsiColor),
}

/// 用户手动 tab 色选择 [`SelectedTabColor`] 的 POD 镜像。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavSelectedTabColor {
    /// 无手动覆盖 — 回落到默认目录色。
    #[default]
    Unset,
    /// 用户显式清除 (覆盖任何默认)。
    Cleared,
    /// 用户显式选定该色。
    Color(NavAnsiColor),
}

impl NavSelectedTabColor {
    /// 解析生效色:手动选择优先,无覆盖时回落 `default`。
    pub fn resolve(self, default: Option<NavAnsiColor>) -> Option<NavAnsiColor> {
        match self {
            Self::Color(c) => Some(c),
            Self::Cleared => None,
            Self::Unset => default,
        }
    }
}

/// [`NavDataSource::pane_dir_color_seed`] / [`color_for_directory_seed`]
/// 消费的目录色配置快照:`(目录路径字符串, 颜色镜像)` 的保序切片。
/// app 侧从 `TabSettings.directory_tab_colors` 投影而来。
pub type DirectoryColorConfig = [(String, NavDirectoryColor)];

/// 目录色查表 — 纯函数 (无 IO)。
///
/// 对 `seed` (cwd/文件路径字符串) 做最长前缀匹配:遍历 `config`,
/// 跳过 [`NavDirectoryColor::Suppressed`] 条目,取前缀命中的最长者;
/// 等长时取后者 (与 app 侧 `max_by_key` 语义一致)。命中
/// [`NavDirectoryColor::Unassigned`] 视为无色 (`None`)。
///
/// 注意:app 侧原实现内部会对路径 `canonicalize` (IO);本纯函数不做,
/// 需要规范化语义的调用方应在传入前自行 canonicalize。
pub fn color_for_directory_seed(seed: &str, config: &DirectoryColorConfig) -> Option<NavAnsiColor> {
    let seed_path = Path::new(seed);
    config
        .iter()
        .filter_map(|(configured_path, color)| {
            let configured = Path::new(configured_path);
            match color {
                NavDirectoryColor::Suppressed => None,
                _ => seed_path.starts_with(configured).then_some((configured, color)),
            }
        })
        .max_by_key(|(configured, _)| configured.as_os_str().len())
        .and_then(|(_, color)| match color {
            NavDirectoryColor::Color(c) => Some(*c),
            NavDirectoryColor::Suppressed | NavDirectoryColor::Unassigned => None,
        })
}

/// 单个 tab 的只读投影。`usize` 索引与宿主 `tabs` 顺序一致,由
/// [`NavDataSource`] 按索引取投影,不暴露 `TabData` 本体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavTabInfo {
    /// Tab 级自定义标题 (rename-tab 流程写入);`None` = 未设置。
    pub custom_title: Option<String>,
    /// 所属项目路径;`None` = 无项目 (始终可见)。
    pub project_path: Option<String>,
    /// 该 tab 是否被用户手动拖拽中。
    pub is_dragging: bool,
    /// pane_group 实体 id — pane 行定位/右键菜单锚点用
    /// (vtab `PaneViewLocator { pane_group_id, .. }` 的镜像源)。
    pub pane_group_id: EntityId,
    /// tab 生效显示标题 = `custom_title` 优先,否则聚焦 pane 生成的
    /// 标题 (vtab `PaneGroup::display_title` 镜像语义);空串 = 未命名。
    pub display_title: String,
    /// 用户手动选定的 tab 色 (右侧色点菜单)。
    pub selected_color: NavSelectedTabColor,
    /// 由活动终端 cwd 派生的目录色 (vtab `sync_codebase_tab_color` 缓存)。
    pub default_directory_color: Option<NavAnsiColor>,
    /// 该 tab 是否为当前激活 tab。
    pub is_focused_tab: bool,
}

/// 单个 pane 的只读投影 — vtab pane 行粒度渲染的数据面。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavPaneInfo {
    /// pane 标识 (pane 类型 + 底层 PaneView 实体 id 的 POD 包装)。
    pub pane_id: PaneId,
    /// pane 类型镜像 — `IPaneType` 本身已是纯枚举 (无 payload),直接复用。
    pub pane_type: IPaneType,
    /// 该 pane 是否为其所属 pane_group 的聚焦 pane。
    pub is_focused: bool,
    /// pane 在 tab 内的展示序 (与 `visible_pane_ids` 顺序一致)。
    pub position_in_tab: usize,
}

impl NavPaneInfo {
    /// pane_group 实体 id 由调用方随 [`NavTabInfo::pane_group_id`] 持有,
    /// pane 行定位时组合 — 这里提供组合便利。
    pub fn locator(&self, pane_group_id: EntityId) -> (EntityId, PaneId) {
        (pane_group_id, self.pane_id)
    }
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
    /// `app` 由渲染端传入 — app 侧实现需读 `ViewHandle` 内容解引用。
    fn tab_info(&self, app: &AppContext, index: usize) -> Option<NavTabInfo>;

    /// 按 flat 索引取该 tab 的 pane 行投影 (vtab pane 粒度渲染数据面)。
    /// 顺序与宿主 `visible_pane_ids` 一致;越界 tab 返回空 `Vec`。
    fn tab_panes(&self, app: &AppContext, tab_index: usize) -> Vec<NavPaneInfo>;

    /// 取 pane 的目录色种子 (terminal = cwd,code = 打开文件路径);
    /// 非 terminal/code pane 或取不到路径返回 `None`。
    /// 渲染端拿种子自行走 [`color_for_directory_seed`] 算色。
    fn pane_dir_color_seed(&self, app: &AppContext, pane_id: PaneId) -> Option<String>;

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

    fn tab_info(&self, _app: &AppContext, _index: usize) -> Option<NavTabInfo> {
        None
    }

    fn tab_panes(&self, _app: &AppContext, _tab_index: usize) -> Vec<NavPaneInfo> {
        Vec::new()
    }

    fn pane_dir_color_seed(&self, _app: &AppContext, _pane_id: PaneId) -> Option<String> {
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
