//! TabProjection — `impl NavDataSource for Workspace` 的 app 侧投影层。
//!
//! 职责:把 `Workspace` 内部状态 (`tabs` / `active_project` / 各种 UI 布尔)
//! 投影成 nav 侧 POD (`NavTabInfo` / `NavPaneInfo` / 颜色镜像),零渲染类型
//! 泄漏 — nav 只见 `EntityId` / `PaneId` / `String` / `bool` / POD 枚举。
//!
//! 边界说明:
//! - `ViewHandle` 解引用需要 `&AppContext`,故带数据的方法都显式收 `app`
//!   参数 (渲染端 vertical_tabs 本就持有 `app`,零额外成本);
//! - `nav` crate 禁止依赖 app,本文件属于 app (warp) 侧,方向合法:
//!   app → nav 单向;
//! - 布尔/枚举字段全部就地投影,不缓存 — 渲染帧内任意次调用幂等。

use std::path::Path;

use nav::data_source::{
    color_for_directory_seed, DirectoryColorConfig, NavAnsiColor, NavDataSource, NavDirectoryColor,
    NavPaneInfo, NavSelectedTabColor, NavTabHover, NavTabInfo, WorkspaceStateSnapshot,
};
use pane_tree::{IPaneType, PaneId};
use crate::tab::SelectedTabColor;
use crate::themes::theme::AnsiColorIdentifier;
use crate::workspace::tab_settings::{DirectoryTabColor, TabSettings};
use crate::workspace::view::Workspace;
use settings::Setting as _;
use warpui::{AppContext, EntityId, SingletonEntity, WindowId};

/// `SelectedTabColor` (app) → `NavSelectedTabColor` (nav 镜像)。
/// 镜像转换唯一入口;`AnsiColorIdentifier` 同步映射。
fn nav_selected_color(value: SelectedTabColor) -> NavSelectedTabColor {
    match value {
        SelectedTabColor::Unset => NavSelectedTabColor::Unset,
        SelectedTabColor::Cleared => NavSelectedTabColor::Cleared,
        SelectedTabColor::Color(c) => NavSelectedTabColor::Color(to_nav_color(c)),
    }
}

/// `AnsiColorIdentifier` (app) → `NavAnsiColor` (nav 镜像)。
fn to_nav_color(value: AnsiColorIdentifier) -> NavAnsiColor {
    match value {
        AnsiColorIdentifier::Black => NavAnsiColor::Black,
        AnsiColorIdentifier::Red => NavAnsiColor::Red,
        AnsiColorIdentifier::Green => NavAnsiColor::Green,
        AnsiColorIdentifier::Yellow => NavAnsiColor::Yellow,
        AnsiColorIdentifier::Blue => NavAnsiColor::Blue,
        AnsiColorIdentifier::Magenta => NavAnsiColor::Magenta,
        AnsiColorIdentifier::Cyan => NavAnsiColor::Cyan,
        AnsiColorIdentifier::White => NavAnsiColor::White,
    }
}

/// `NavAnsiColor` (nav 镜像) → `AnsiColorIdentifier` (app)。
/// 渲染端把 POD 色转回主题色用。
fn from_nav_color(value: NavAnsiColor) -> AnsiColorIdentifier {
    match value {
        NavAnsiColor::Black => AnsiColorIdentifier::Black,
        NavAnsiColor::Red => AnsiColorIdentifier::Red,
        NavAnsiColor::Green => AnsiColorIdentifier::Green,
        NavAnsiColor::Yellow => AnsiColorIdentifier::Yellow,
        NavAnsiColor::Blue => AnsiColorIdentifier::Blue,
        NavAnsiColor::Magenta => AnsiColorIdentifier::Magenta,
        NavAnsiColor::Cyan => AnsiColorIdentifier::Cyan,
        NavAnsiColor::White => AnsiColorIdentifier::White,
    }
}

/// 读当前目录色配置快照 (供纯函数 [`color_for_directory_seed`] 消费)。
/// app 侧也用同一投影 — `TabSettings.directory_tab_colors` →
/// `(路径字符串, NavDirectoryColor)` 保序切片。
pub(crate) fn directory_color_config(app: &AppContext) -> Vec<(String, NavDirectoryColor)> {
    TabSettings::as_ref(app)
        .directory_tab_colors
        .value()
        .0
        .iter()
        .map(|(path, color)| {
            let color = match color {
                DirectoryTabColor::Suppressed => NavDirectoryColor::Suppressed,
                DirectoryTabColor::Unassigned => NavDirectoryColor::Unassigned,
                DirectoryTabColor::Color(c) => NavDirectoryColor::Color(to_nav_color(*c)),
            };
            (path.clone(), color)
        })
        .collect()
}

/// tab 生效显示标题 — 与 `PaneGroup::display_title` 语义一致:
/// 自定义标题优先,否则聚焦 pane 生成标题;两者皆空时为空串。
fn tab_display_title(workspace: &Workspace, index: usize, app: &AppContext) -> String {
    let tab = &workspace.tabs[index];
    let pane_group = tab.pane_group.as_ref(app);
    pane_group.display_title(app)
}

impl NavDataSource for Workspace {
    fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    fn tab_info(&self, app: &AppContext, index: usize) -> Option<NavTabInfo> {
        let tab = self.tabs.get(index)?;
        let pane_group = tab.pane_group.as_ref(app);
        Some(NavTabInfo {
            custom_title: pane_group.custom_title(app),
            project_path: tab
                .project_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            is_dragging: tab.draggable_state.is_dragging(),
            pane_group_id: tab.pane_group.id(),
            display_title: tab_display_title(self, index, app),
            selected_color: nav_selected_color(tab.selected_color),
            default_directory_color: tab.default_directory_color.map(to_nav_color),
            is_focused_tab: index == self.active_tab_index,
        })
    }

    fn tab_panes(&self, app: &AppContext, tab_index: usize) -> Vec<NavPaneInfo> {
        let Some(tab) = self.tabs.get(tab_index) else {
            return Vec::new();
        };
        let pane_group = tab.pane_group.as_ref(app);
        let focused = pane_group.focused_pane_id(app);
        pane_group
            .visible_pane_ids()
            .into_iter()
            .enumerate()
            .map(|(position, pane_id)| NavPaneInfo {
                pane_type: pane_type_of(pane_id),
                pane_id,
                is_focused: pane_id == focused,
                position_in_tab: position,
            })
            .collect()
    }

    fn pane_dir_color_seed(&self, app: &AppContext, pane_id: PaneId) -> Option<String> {
        // 与 vertical_tabs `compute_tab_group_color_mode` 同源:
        // terminal 取 cwd,code 取打开文件路径,其余 pane 无种子。
        for tab in &self.tabs {
            let pane_group = tab.pane_group.as_ref(app);
            if let Some(tv) = pane_group.terminal_view_from_pane_id(pane_id, app) {
                return tv.as_ref(app).pwd_if_local(app);
            }
            if let Some(code_view) = pane_group.code_view_from_pane_id(pane_id, app) {
                return code_view
                    .as_ref(app)
                    .local_path(app)
                    .map(|p| p.to_string_lossy().into_owned());
            }
        }
        None
    }

    fn tab_indices_with_project(&self, project: &Path) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.project_path.as_deref() == Some(project))
            .map(|(index, _)| index)
            .collect()
    }

    fn tab_indices_without_project(&self, known_projects: &[std::path::PathBuf]) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| match &tab.project_path {
                // 无归属且不在任何已知项目下 → "未分组"。
                Some(p) => !known_projects.contains(p),
                None => true,
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn visible_tab_indices(&self) -> Vec<usize> {
        // 与 `Workspace::visible_tab_indices` 同语义 (项目过滤器)。
        match &self.active_project {
            None => (0..self.tabs.len()).collect(),
            Some(project) => self
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| match &tab.project_path {
                    None => true,
                    Some(path) => path == project,
                })
                .map(|(index, _)| index)
                .collect(),
        }
    }

    fn active_tab_index(&self) -> usize {
        self.active_tab_index
    }

    fn active_project(&self) -> Option<&Path> {
        self.active_project.as_deref()
    }

    fn hovered_tab_index(&self) -> Option<NavTabHover> {
        self.hovered_tab_index.map(|h| match h {
            crate::pane_group::TabBarHoverIndex::BeforeTab(i) => NavTabHover::BeforeTab(i),
            crate::pane_group::TabBarHoverIndex::OverTab(i) => NavTabHover::OverTab(i),
        })
    }

    fn window_id(&self) -> WindowId {
        self.window_id
    }

    fn current_workspace_state(&self) -> WorkspaceStateSnapshot {
        let s = &self.current_workspace_state;
        WorkspaceStateSnapshot {
            is_agent_management_view_open: s.is_agent_management_view_open,
            is_tab_being_renamed: s.is_tab_being_renamed(),
            tab_being_renamed: s.tab_being_renamed(),
            is_any_pane_being_renamed: s.is_any_pane_being_renamed(),
        }
    }

    fn show_new_session_dropdown_menu(&self) -> bool {
        self.show_new_session_dropdown_menu.is_some()
    }

    fn is_tab_context_menu_open(&self, tab_index: usize) -> bool {
        self.show_tab_right_click_menu
            .is_some_and(|(idx, _)| idx == tab_index)
    }

    fn tab_rename_editor_active(&self) -> bool {
        // 编辑器 handle 恒存在,存在性即活跃性 (trait v1 语义:bool 表达)。
        true
    }

    fn pane_rename_editor_active(&self) -> bool {
        true
    }
}

/// `PaneId` → `IPaneType` 镜像 (pane_tree 本就是纯类型,直接透传)。
fn pane_type_of(pane_id: PaneId) -> IPaneType {
    pane_id.pane_type()
}

/// tab 的 `pane_group` 实体 id — vtab `PaneViewLocator` 组装用。
/// (trait 外便利函数;`NavTabInfo.pane_group_id` 已携带同值。)
pub(crate) fn pane_group_id_of(workspace: &Workspace, index: usize) -> Option<EntityId> {
    workspace.tabs.get(index).map(|tab| tab.pane_group.id())
}

/// 供渲染端按种子算目录色的纯函数 re-export 入口。
/// 用法:`directory_color_config(app)` → `color_for_directory_seed(seed, &config)`。
pub(crate) fn seed_color_of(seed: &str, config: &DirectoryColorConfig) -> Option<NavAnsiColor> {
    color_for_directory_seed(seed, config)
}
