//! panel_api — 头部工具栏面板条目的类型与注册表。
//!
//! 布局拆分 v3a (split-plan §4): `HeaderToolbarItemKind` 自 app 迁入 nav,
//! app 侧 `workspace::header_toolbar_item` 保留 re-export 壳以兼容既有
//! settings 序列化路径 (TabSettings / HeaderToolbarChipSelection)。
//!
//! 职责边界:
//! - nav 持有"条目是什么"(kind / 图标 / 可用性判定 / 展示元数据);
//! - app 持有"按钮怎么画"(渲染走 `PanelHost` trait, 宿主实现按钮工厂)。
//!   nav 不依赖 app, 渲染回调以 `&dyn PanelHost` 注入。
//!
//! `PanelRegistry` 为 Vec 注册表: 条目数量小 (≤8), 线性查找即够;
//! 注册表是常量知识, 无运行时可变输入, 直接 `OnceLock` 静态化 (rs-lazylock
//! 规则允许 OnceLock 承载常量构造)。
// `local_fs` 是 app 侧 feature, 不在 nav 的 feature 空间 —
// `cfg!(feature = "local_fs")` 按值展开 (恒编译期常量), 警告在此允许。
#![allow(unexpected_cfgs)]
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::AppContext;

/// 头部工具栏可配置条目。
///
/// 自 app `workspace/header_toolbar_item.rs` 迁入 (v3a)。serde/schemars
/// 派生与原实现逐字段一致 — 持久化的 chip selection 格式不受迁移影响。
pub mod header_toolbar_item_kind {
    use serde::{Deserialize, Serialize};

    use warpui::AppContext;

    /// A configurable item in the vertical tabs header toolbar.
    ///
    /// Each variant represents a panel toggle button that can be placed on either
    /// the left or right side of the toolbar. The side determines which side of the
    /// main content area the panel opens on.
    #[derive(
        Clone,
        Debug,
        Eq,
        PartialEq,
        Hash,
        Serialize,
        Deserialize,
        schemars::JsonSchema,
        settings_value::SettingsValue,
    )]
    #[schemars(rename_all = "snake_case")]
    pub enum HeaderToolbarItemKind {
        TabsPanel,
        ToolsPanel,
        AgentManagement,
        CodeReview,
        /// 观测台面板（Observatory）— 拦截会话 + 编排状态。
        Observatory,
        /// Cockpit 面板 — 多 agent 终端驾驶舱(hub-tui 原生移植)。
        #[cfg(not(target_family = "wasm"))]
        Cockpit,
        /// Notifications mailbox.
        NotificationsMailbox,
    }

    impl HeaderToolbarItemKind {
        /// Whether this item is supported on the current platform/configuration
        /// (feature flags, compile-time features, AI enabled, auth state).
        /// Does not check user show/hide preferences — use `is_available` for that.
        ///
        /// 可用性判定依赖运行时 settings (TabSettings/AISettings, app 侧类
        /// 型) — nav 不依赖 app, 故经 `super::availability` 注入的函数指针
        /// 读取。app 在初始化时 `set_availability_hooks` 一次性挂接。
        pub fn is_supported(&self, app: &AppContext) -> bool {
            match self {
                Self::TabsPanel => {
                    super::feature_flag_tabs_enabled()
                        && (super::availability().use_vertical_tabs)(app)
                }
                Self::ToolsPanel => true,
                Self::AgentManagement => false,
                Self::Observatory => super::feature_flag_agent_harness(),
                #[cfg(not(target_family = "wasm"))]
                Self::Cockpit => super::feature_flag_agent_harness(),
                // app 侧 feature `local_fs` 不在 nav 的 feature 空间 —
                // unexpected_cfg 警告在本 crate 顶部允许。
                Self::CodeReview => cfg!(feature = "local_fs"),
                Self::NotificationsMailbox => super::feature_flag_hoa_notifications(),
            }
        }

        /// Whether this item should be shown in the toolbar.
        /// Checks both `is_supported` and user show/hide preferences.
        pub fn is_available(&self, app: &AppContext) -> bool {
            if !self.is_supported(app) {
                return false;
            }
            match self {
                Self::CodeReview => (super::availability().show_code_review_button)(app),
                Self::NotificationsMailbox => {
                    (super::availability().show_agent_notifications)(app)
                }
                _ => true,
            }
        }
    }
}

pub use header_toolbar_item_kind::HeaderToolbarItemKind;

/// 用户显隐偏好读取钩子 — app 侧注入 (settings 类型属 app, nav 不依赖)。
///
/// 三个 fn 指针在 app 初始化时经 [`set_availability_hooks`] 一次性挂接;
/// 未挂接时 `is_available` 的偏好分支按"不可用"处理 (安全默认)。
#[derive(Copy, Clone)]
pub struct AvailabilityHooks {
    pub use_vertical_tabs: fn(&AppContext) -> bool,
    pub show_code_review_button: fn(&AppContext) -> bool,
    pub show_agent_notifications: fn(&AppContext) -> bool,
}

static AVAILABILITY_HOOKS: std::sync::LazyLock<std::sync::OnceLock<AvailabilityHooks>> =
    std::sync::LazyLock::new(std::sync::OnceLock::new);

/// app 初始化时挂接偏好读取钩子; 幂等 (首次生效)。
///
/// 钩子是运行时注入 (app 侧函数指针编译期未知), 属 rs-lazylock 规则允许
/// OnceLock 的场景; LazyLock 仅用于承载静态单元本身。
pub fn set_availability_hooks(hooks: AvailabilityHooks) {
    let _ = AVAILABILITY_HOOKS.set(hooks);
}

fn availability() -> AvailabilityHooks {
    *AVAILABILITY_HOOKS.get().unwrap_or(&AvailabilityHooks {
        use_vertical_tabs: |_| false,
        show_code_review_button: |_| false,
        show_agent_notifications: |_| false,
    })
}

// FeatureFlag 包装 — 避免在 match 分支里散布 cfg 时语义漂移,
// 同时让 wasm 侧裁剪集中在 warp_core 特性门内。
fn feature_flag_tabs_enabled() -> bool {
    warp_core::features::FeatureFlag::VerticalTabs.is_enabled()
}

fn feature_flag_agent_harness() -> bool {
    warp_core::features::FeatureFlag::AgentHarness.is_enabled()
}

fn feature_flag_hoa_notifications() -> bool {
    warp_core::features::FeatureFlag::HOANotifications.is_enabled()
}

/// 面板条目渲染宿主 — app 侧 (Workspace) 实现。
///
/// 每个方法渲染一个面板开关按钮; nav 侧查表后回调, 不感知 app 类型。
pub trait PanelHost {
    /// 左栏开关按钮 (旧 TabsPanel 位, 常驻左栏的收起态开关)。
    fn render_left_toggle(&self, appearance: &Appearance, ctx: &AppContext)
    -> Box<dyn warpui::Element>;
    /// 工具箱面板按钮。
    fn render_tools_panel(&self, appearance: &Appearance, ctx: &AppContext)
    -> Box<dyn warpui::Element>;
    /// Agent 管理入口按钮 (BYOP 后为空实现)。
    fn render_agent_management(&self, appearance: &Appearance, ctx: &AppContext)
    -> Box<dyn warpui::Element>;
    /// 右栏 (Code Review) 按钮。
    fn render_right_panel(&self, appearance: &Appearance, ctx: &AppContext)
    -> Box<dyn warpui::Element>;
    /// 观测台按钮。
    fn render_observatory(&self, appearance: &Appearance, ctx: &AppContext)
    -> Box<dyn warpui::Element>;
    /// Cockpit 按钮 (仅非 wasm)。
    #[cfg(not(target_family = "wasm"))]
    fn render_cockpit(&self, appearance: &Appearance, ctx: &AppContext)
    -> Box<dyn warpui::Element>;
    /// 通知邮箱按钮。
    fn render_notifications_mailbox(
        &self,
        appearance: &Appearance,
        ctx: &AppContext,
    ) -> Box<dyn warpui::Element>;
}

/// 单个面板条目的静态描述。
///
/// - `name`: 调试/日志用稳定标识。
/// - `available`: 可用性判定 (特性开关 + 用户显隐偏好), 由 app 侧迁移的
///   `HeaderToolbarItemKind::is_available` 提供。
/// - `render`: 按钮渲染工厂 — 从宿主取对应按钮。
pub struct PanelDescriptor {
    pub name: &'static str,
    pub available: fn(&HeaderToolbarItemKind, &AppContext) -> bool,
    pub render: fn(&dyn PanelHost, &HeaderToolbarItemKind, &Appearance, &AppContext)
        -> Box<dyn warpui::Element>,
}

/// 面板注册表 — Vec 承载, `get` 线性查找。
pub struct PanelRegistry {
    entries: Vec<(HeaderToolbarItemKind, PanelDescriptor)>,
}

impl PanelRegistry {
    /// 从条目列表构建注册表。
    pub fn new(entries: Vec<(HeaderToolbarItemKind, PanelDescriptor)>) -> Self {
        Self { entries }
    }

    /// 按条目取描述; 未注册返回 `None`。
    pub fn get(&self, kind: &HeaderToolbarItemKind) -> Option<&PanelDescriptor> {
        self.entries
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, d)| d)
    }

    /// 已注册条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 默认注册表 — 全部条目 + 按钮工厂 (回调宿主的对应 render 方法)。
///
/// 常量知识 (无运行时输入), LazyLock 静态化 (rs-lazylock)。
pub fn default_panel_registry() -> &'static PanelRegistry {
    static REGISTRY: std::sync::LazyLock<PanelRegistry> = std::sync::LazyLock::new(|| {
        PanelRegistry::new(vec![
            (
                HeaderToolbarItemKind::TabsPanel,
                PanelDescriptor {
                    name: "tabs_panel",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| host.render_left_toggle(appearance, ctx),
                },
            ),
            (
                HeaderToolbarItemKind::ToolsPanel,
                PanelDescriptor {
                    name: "tools_panel",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| host.render_tools_panel(appearance, ctx),
                },
            ),
            (
                HeaderToolbarItemKind::AgentManagement,
                PanelDescriptor {
                    name: "agent_management",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| {
                        host.render_agent_management(appearance, ctx)
                    },
                },
            ),
            (
                HeaderToolbarItemKind::CodeReview,
                PanelDescriptor {
                    name: "code_review",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| host.render_right_panel(appearance, ctx),
                },
            ),
            (
                HeaderToolbarItemKind::Observatory,
                PanelDescriptor {
                    name: "observatory",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| host.render_observatory(appearance, ctx),
                },
            ),
            #[cfg(not(target_family = "wasm"))]
            (
                HeaderToolbarItemKind::Cockpit,
                PanelDescriptor {
                    name: "cockpit",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| host.render_cockpit(appearance, ctx),
                },
            ),
            (
                HeaderToolbarItemKind::NotificationsMailbox,
                PanelDescriptor {
                    name: "notifications_mailbox",
                    available: |k, app| k.is_available(app),
                    render: |host, _, appearance, ctx| {
                        host.render_notifications_mailbox(appearance, ctx)
                    },
                },
            ),
        ])
    });
    &REGISTRY
}

/// 默认图标 (条目迁入 nav 后的 icon 表, 供配置器等元数据消费方使用)。
///
/// 从 app `header_toolbar_item.rs::icon` 原样迁入。
impl HeaderToolbarItemKind {
    pub fn icon(&self) -> Icon {
        match self {
            Self::TabsPanel => Icon::Menu,
            Self::ToolsPanel => Icon::Tool2,
            Self::AgentManagement => Icon::Grid,
            Self::CodeReview => Icon::Diff,
            Self::Observatory => Icon::Eye,
            #[cfg(not(target_family = "wasm"))]
            // D7: Grid 与 AgentManagement(Grid) 撞图标; Rocket 全 app 零占用,
            // 贴驾驶舱语义。
            Self::Cockpit => Icon::Rocket,
            Self::NotificationsMailbox => Icon::Inbox,
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::TabsPanel => "Tabs Panel",
            Self::ToolsPanel => "Tools Panel",
            Self::AgentManagement => "Agent Management",
            Self::CodeReview => "Code Review",
            Self::Observatory => "Observatory",
            #[cfg(not(target_family = "wasm"))]
            Self::Cockpit => "Cockpit",
            Self::NotificationsMailbox => "Notifications",
        }
    }

    /// 是否为侧栏面板型条目 (相对替换内容区/弹 popover 的条目)。
    pub fn is_panel(&self) -> bool {
        matches!(
            self,
            Self::TabsPanel | Self::ToolsPanel | Self::CodeReview | Self::Observatory
        )
    }

    pub fn default_left() -> Vec<Self> {
        // 2026-08 GUI 重构: header 只留观测台 + cockpit。TabsPanel(左栏收起)
        // /ToolsPanel/AgentManagement 从默认集退役——左栏改为 cockpit 驱动的
        // 常驻导航(不可收起),工具箱入口移至 rail footer。
        vec![
            Self::Observatory,
            #[cfg(not(target_family = "wasm"))]
            Self::Cockpit,
        ]
    }

    pub fn default_right() -> Vec<Self> {
        vec![Self::CodeReview, Self::NotificationsMailbox]
    }

    pub fn all_items() -> Vec<Self> {
        // 2026-08 GUI 重构: TabsPanel/ToolsPanel/AgentManagement 退役,
        // 配置器不再提供(左栏常驻 + 工具箱移 rail footer)。
        vec![
            Self::CodeReview,
            Self::Observatory,
            #[cfg(not(target_family = "wasm"))]
            Self::Cockpit,
            Self::NotificationsMailbox,
        ]
    }
}
