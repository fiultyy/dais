//! NavDataSource 数据面幂等测试。
//!
//! 锁定两组不变量 (拆分契约 v2b' 验收项 4):
//! 1. `NavTabInfo` / `NavPaneInfo` serde round-trip byte-equal —
//!    POD 投影未来跨 进程/存储/网络 边界时序列化必须恒等;
//! 2. `tab_indices_with_project` / `tab_indices_without_project` /
//!    `visible_tab_indices` 同输入同输出 — 过滤函数确定性。
//!
//! fixture 用 nav 内部最小数据源 (`FixtureSource`),不依赖 app;
//! trait 的 `&AppContext` 参数在 fixture 实现里全部 `_app` 忽略,
//! 测试走不触 app 的内部包装方法,避免在测试中构造运行时。

use std::path::{Path, PathBuf};

use nav::data_source::{
    color_for_directory_seed, NavAnsiColor, NavDataSource, NavDirectoryColor, NavPaneInfo,
    NavSelectedTabColor, NavTabInfo, WorkspaceStateSnapshot,
};
use pane_tree::{IPaneType, PaneId};
use warpui::{EntityId, WindowId};

// ---------------------------------------------------------------------------
// fixture
// ---------------------------------------------------------------------------

/// 最小确定性数据源:tab 项目路径表,索引即顺序。
struct FixtureSource {
    projects: Vec<Option<PathBuf>>,
    active_project: Option<PathBuf>,
}

impl FixtureSource {
    fn tab_count(&self) -> usize {
        self.projects.len()
    }

    /// 确定性构造 tab 投影 (与 trait 实现同一逻辑,不触 app)。
    fn tab_info_inner(&self, index: usize) -> Option<NavTabInfo> {
        let project = self.projects.get(index)?;
        Some(NavTabInfo {
            custom_title: (index % 2 == 0).then(|| format!("tab-{index}")),
            project_path: project
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            is_dragging: false,
            pane_group_id: EntityId::from_usize(1000 + index),
            display_title: format!("display-{index}"),
            selected_color: if index % 3 == 0 {
                NavSelectedTabColor::Color(NavAnsiColor::Green)
            } else {
                NavSelectedTabColor::Unset
            },
            default_directory_color: (index % 2 == 1).then_some(NavAnsiColor::Magenta),
            is_focused_tab: index == 1,
        })
    }

    /// 确定性构造 pane 行投影:每 tab 两个 pane (terminal + code)。
    fn tab_panes_inner(&self, tab_index: usize) -> Vec<NavPaneInfo> {
        if tab_index >= self.projects.len() {
            return Vec::new();
        }
        vec![
            NavPaneInfo {
                pane_id: PaneId::new(IPaneType::Terminal, EntityId::from_usize(10 + tab_index)),
                pane_type: IPaneType::Terminal,
                is_focused: true,
                position_in_tab: 0,
            },
            NavPaneInfo {
                pane_id: PaneId::new(IPaneType::Code, EntityId::from_usize(20 + tab_index)),
                pane_type: IPaneType::Code,
                is_focused: false,
                position_in_tab: 1,
            },
        ]
    }
}

impl NavDataSource for FixtureSource {
    fn tab_count(&self) -> usize {
        FixtureSource::tab_count(self)
    }

    fn tab_info(&self, _app: &warpui::AppContext, index: usize) -> Option<NavTabInfo> {
        self.tab_info_inner(index)
    }

    fn tab_panes(&self, _app: &warpui::AppContext, tab_index: usize) -> Vec<NavPaneInfo> {
        self.tab_panes_inner(tab_index)
    }

    fn pane_dir_color_seed(&self, _app: &warpui::AppContext, _pane_id: PaneId) -> Option<String> {
        Some("/fixture/repo".to_owned())
    }

    fn tab_indices_with_project(&self, project: &Path) -> Vec<usize> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, p)| p.as_deref() == Some(project))
            .map(|(i, _)| i)
            .collect()
    }

    fn tab_indices_without_project(&self, known_projects: &[PathBuf]) -> Vec<usize> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, p)| match p {
                Some(path) => !known_projects.contains(path),
                None => true,
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn visible_tab_indices(&self) -> Vec<usize> {
        match &self.active_project {
            None => (0..self.projects.len()).collect(),
            Some(project) => self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| match p {
                    None => true,
                    Some(path) => path == project,
                })
                .map(|(i, _)| i)
                .collect(),
        }
    }

    fn active_tab_index(&self) -> usize {
        1
    }

    fn active_project(&self) -> Option<&Path> {
        self.active_project.as_deref()
    }

    fn hovered_tab_index(&self) -> Option<nav::data_source::NavTabHover> {
        None
    }

    fn window_id(&self) -> WindowId {
        WindowId::from_usize(7)
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

fn fixture_source() -> FixtureSource {
    FixtureSource {
        projects: vec![
            Some(PathBuf::from("/repo/alpha")),
            Some(PathBuf::from("/repo/beta")),
            None,
            Some(PathBuf::from("/repo/alpha")),
            None,
        ],
        active_project: None,
    }
}

// ---------------------------------------------------------------------------
// serde round-trip byte-equal
// ---------------------------------------------------------------------------

/// serde round-trip byte-equal 断言:同值两次 serialize 字节一致,
/// 且 deserialize(serialize(x)) 的再序列化也 byte-equal。
fn assert_roundtrip_byte_equal<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let bytes_a = serde_json::to_vec(value).expect("serialize a");
    let bytes_b = serde_json::to_vec(value).expect("serialize b");
    assert_eq!(bytes_a, bytes_b, "同值两次序列化字节不同 (非确定性)");
    let back: T = serde_json::from_slice(&bytes_a).expect("deserialize");
    let bytes_back = serde_json::to_vec(&back).expect("serialize roundtrip");
    assert_eq!(
        bytes_a, bytes_back,
        "round-trip 后再序列化字节不同 (非恒等)"
    );
    assert_eq!(&back, value, "round-trip 后值不等");
}

#[test]
fn nav_tab_info_serde_roundtrip_byte_equal() {
    let source = fixture_source();
    for index in 0..source.tab_count() {
        let info = source.tab_info_inner(index).expect("in bounds");
        assert_roundtrip_byte_equal(&info);
    }
}

#[test]
fn nav_pane_info_serde_roundtrip_byte_equal() {
    let source = fixture_source();
    for tab_index in 0..source.tab_count() {
        for pane in source.tab_panes_inner(tab_index) {
            assert_roundtrip_byte_equal(&pane);
        }
    }
}

#[test]
fn nav_ansi_color_u8_mirror_stable() {
    // 镜像 u8 视图稳定:as_u8/from_u8 全变体往返恒等,越界拒绝。
    let all = [
        NavAnsiColor::Black,
        NavAnsiColor::Red,
        NavAnsiColor::Green,
        NavAnsiColor::Yellow,
        NavAnsiColor::Blue,
        NavAnsiColor::Magenta,
        NavAnsiColor::Cyan,
        NavAnsiColor::White,
    ];
    for (i, c) in all.iter().enumerate() {
        assert_eq!(c.as_u8(), i as u8);
        assert_eq!(NavAnsiColor::from_u8(i as u8), Some(*c));
        assert_roundtrip_byte_equal(c);
    }
    assert_eq!(NavAnsiColor::from_u8(8), None);
}

// ---------------------------------------------------------------------------
// 过滤函数确定性
// ---------------------------------------------------------------------------

#[test]
fn tab_filter_functions_deterministic() {
    let source = fixture_source();
    let alpha = Path::new("/repo/alpha");

    // 同输入多次调用结果一致。
    for _ in 0..3 {
        assert_eq!(source.tab_indices_with_project(alpha), vec![0, 3]);
        assert_eq!(
            source.tab_indices_without_project(&[PathBuf::from("/repo/alpha")]),
            vec![1, 2, 4]
        );
        assert_eq!(source.visible_tab_indices(), vec![0, 1, 2, 3, 4]);
    }

    // 选中项目过滤器后可见集缩窄且保序。
    let mut filtered = fixture_source();
    filtered.active_project = Some(PathBuf::from("/repo/alpha"));
    // 无项目 tab 恒可见 (不被隐藏),项目 tab 过滤保留 — 与 app 侧
    // `Workspace::visible_tab_indices` 同语义: [0(alpha),2(None),3(alpha),4(None)]。
    assert_eq!(filtered.visible_tab_indices(), vec![0, 2, 3, 4]);

    // 过滤函数纯度:调用后状态不被修改 (再查一遍仍一致)。
    assert_eq!(source.tab_indices_with_project(alpha), vec![0, 3]);
    assert_eq!(source.tab_count(), 5);
}

// ---------------------------------------------------------------------------
// color_for_directory_seed 纯函数
// ---------------------------------------------------------------------------

#[test]
fn color_for_directory_seed_longest_prefix_wins() {
    let config: Vec<(String, NavDirectoryColor)> = vec![
        (
            "/repo".to_owned(),
            NavDirectoryColor::Color(NavAnsiColor::Blue),
        ),
        (
            "/repo/alpha".to_owned(),
            NavDirectoryColor::Color(NavAnsiColor::Red),
        ),
        (
            "/repo/alpha/deep".to_owned(),
            NavDirectoryColor::Suppressed,
        ),
        ("/other".to_owned(), NavDirectoryColor::Unassigned),
    ];

    // 最长前缀命中。
    assert_eq!(
        color_for_directory_seed("/repo/alpha/src", &config),
        Some(NavAnsiColor::Red)
    );
    // 次长前缀回落。
    assert_eq!(
        color_for_directory_seed("/repo/beta", &config),
        Some(NavAnsiColor::Blue)
    );
    // Suppressed 条目被完全跳过,深层路径回落到最近的可匹配前缀。
    assert_eq!(
        color_for_directory_seed("/repo/alpha/deep/file.rs", &config),
        Some(NavAnsiColor::Red)
    );
    // Unassigned 显式无色。
    assert_eq!(color_for_directory_seed("/other/x", &config), None);
    // 无命中。
    assert_eq!(color_for_directory_seed("/nomatch", &config), None);
}

#[test]
fn color_for_directory_seed_deterministic_on_equal_input() {
    let config: Vec<(String, NavDirectoryColor)> = vec![
        ("/a".to_owned(), NavDirectoryColor::Color(NavAnsiColor::Cyan)),
        (
            "/a".to_owned(),
            NavDirectoryColor::Color(NavAnsiColor::Yellow),
        ),
    ];
    // 等长前缀重复条目:两次同输入同输出 (确定性,而非崩溃)。
    let first = color_for_directory_seed("/a/file", &config);
    let second = color_for_directory_seed("/a/file", &config);
    assert_eq!(first, second);
}
