//! Pane 标识类型底座 (布局拆分 v1 步骤3, 自 app/src/pane_group/pane/mod.rs
//! 下沉 id 层; IPaneType 变体名/Display 与原版逐字一致)。
//!
//! 设计约束(原注释保留): 不允许消费者从 PaneId 推导底层 view id —— pane
//! 内容访问必须经 PaneGroup API。`v1` 只放 id 层; 树结构 (PaneNode/Region)
//! v2 迁入本 crate。
//!
//! 幂等测试锁定: serde round-trip (Serialize→Deserialize 恒等) / Display
//! 稳定性 / Ord 全序(HashMap/排序依赖)。

pub mod snapshot;
pub use snapshot::{BranchSnapshot, LeafSnapshot, PaneFlex, PaneNodeSnapshot, SplitDirection};

use std::fmt::Display;

use serde::{Deserialize, Serialize};
use warpui::EntityId;

/// A [`PaneId`] that is known to belong to a terminal pane.
/// Generally, prefer [`PaneId`], except for logic/features that will only
/// ever apply to terminal sessions (like synced inputs and the block-sharing modal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerminalPaneId(pub EntityId);

impl From<TerminalPaneId> for PaneId {
    fn from(terminal_pane: TerminalPaneId) -> Self {
        PaneId(IPaneId {
            pane_type: IPaneType::Terminal,
            pane_view_id: terminal_pane.0,
        })
    }
}

impl TerminalPaneId {
    /// Creates a [`TerminalPaneId`] for a dummy terminal pane (测试专用)。
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn dummy_terminal_pane_id() -> Self {
        Self(EntityId::new())
    }
}

/// An internal representation of a pane ID. Specifically, we don't want to allow
/// consumers to derive the underlying view ID from a pane ID. Instead, consumers
/// should use the relevant PaneGroup APIs to access pane content (which
/// can provide the underlying view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneId(pub(crate) IPaneId);

impl PaneId {
    /// 组装 pane id (pane 类型 + PaneView 实体 id)。
    /// app 侧 `PaneGroup` 构造各 pane id 的唯一入口 (原 `Self(IPaneId {...})`)。
    pub fn new(pane_type: IPaneType, pane_view_id: EntityId) -> Self {
        Self(IPaneId {
            pane_type,
            pane_view_id,
        })
    }

    /// Creates a [`PaneId`] for a dummy pane (测试专用; 无生产构造点)。
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn dummy_pane_id() -> Self {
        Self(IPaneId {
            pane_type: IPaneType::Dummy,
            pane_view_id: EntityId::new(),
        })
    }

    pub fn terminal_pane_id(&self) -> Option<TerminalPaneId> {
        self.as_terminal_pane_id()
    }

    /// Returns a [`TerminalPaneId`] for the pane, if this is a terminal pane ID.
    pub fn as_terminal_pane_id(&self) -> Option<TerminalPaneId> {
        if matches!(self.0.pane_type, IPaneType::Terminal) {
            Some(TerminalPaneId(self.0.pane_view_id))
        } else {
            None
        }
    }

    pub fn pane_type(&self) -> IPaneType {
        self.0.pane_type
    }

    /// 创建顺序 (EntityId 单调) — tab 恢复排序依赖。
    pub fn creation_order_id(&self) -> EntityId {
        self.0.pane_view_id
    }

    /// 底层 PaneView 实体 id (pane_tree 内部消费; 外部禁止派生 view)。
    pub fn pane_view_id(&self) -> EntityId {
        self.0.pane_view_id
    }

    pub fn is_terminal_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Terminal)
    }

    pub fn is_notebook_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Notebook)
    }

    pub fn is_code_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Code)
    }

    pub fn is_file_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::File)
    }

    pub fn is_code_diff_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::CodeDiff)
    }

    pub fn is_env_var_collection_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::EnvVarCollection)
    }

    pub fn is_settings_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Settings)
    }

    pub fn is_ai_fact_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::AIFact)
    }

    pub fn is_ai_document_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::AIDocument)
    }

    pub fn is_execution_profile_editor_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::ExecutionProfileEditor)
    }

    pub fn is_ssh_server_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::SshServer)
    }

    pub fn is_sftp_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Sftp)
    }

    /// Dais Wave 7-3:ambient-agent UI 子系统物理删,任意 pane 都不是
    /// environment management pane。调用者为渐进式清理保留、返回 false。
    pub fn is_environment_management_pane(&self) -> bool {
        false
    }

    pub fn is_observatory_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Observatory)
    }

    pub fn is_cockpit_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Cockpit)
    }

    pub fn is_welcome_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::Welcome)
    }

    pub fn is_get_started_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::GetStarted)
    }

    pub fn is_deferred_placeholder_pane(&self) -> bool {
        matches!(self.0.pane_type, IPaneType::DeferredPlaceholder)
    }

    pub fn position_id(&self) -> String {
        self.to_string()
    }
}

impl Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pane {} ({})", self.0.pane_type, self.0.pane_view_id)
    }
}

/// An internal representation of a pane ID. Specifically, we don't want to allow
/// consumers to derive the underlying view ID from a pane ID. Instead, consumers
/// should use the relevant PaneGroup APIs to access pane content (which
/// can provide the underlying view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct IPaneId {
    /// The type of pane. Needs to match the BackingView.
    pub(crate) pane_type: IPaneType,

    /// The entity id of the PaneView<BackingView>.
    pub(crate) pane_view_id: EntityId,
}

impl Display for IPaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pane {} ({})", self.pane_type, self.pane_view_id)
    }
}

/// The type of a pane. Needs to match the BackingView.
///
/// 变体集合含全部业务 pane;`v1` 仅类型迁移,不带业务 payload(原设计即无 payload:
/// 全部 unit variant), nav/left_rail 等 id 消费者随迁无阻。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IPaneType {
    Terminal,
    Notebook,
    File,
    ImageViewer,
    Code,
    CodeDiff,
    EnvVarCollection,
    Workflow,
    Settings,
    AIFact,
    AIDocument,
    ExecutionProfileEditor,
    SshServer,
    Sftp,
    Observatory,
    GetStarted,
    /// 多 agent 终端驾驶舱 pane(hub-tui 原生移植;cfg 门控同 Observatory:
    /// 变体本身不 cfg,PaneId 构造/render 臂 cfg)。
    Cockpit,
    Welcome,
    DeferredPlaceholder,
    /// A pane type only for tests (原 app #[cfg(test)] 变体; app 的 cfg(test)
    /// 不影响本 crate 编译, 故无条件保留 — 无生产构造点, dead_code 允许)。
    #[allow(dead_code)]
    Dummy,
}

impl Display for IPaneType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IPaneType::Terminal => write!(f, "Terminal"),
            IPaneType::Notebook => write!(f, "Notebook"),
            IPaneType::File => write!(f, "File"),
            IPaneType::ImageViewer => write!(f, "Image Viewer"),
            IPaneType::Code => write!(f, "Code"),
            IPaneType::CodeDiff => write!(f, "Code Diff"),
            IPaneType::EnvVarCollection => write!(f, "Environment Variable Collection"),
            IPaneType::Workflow => write!(f, "Workflow"),
            IPaneType::Settings => write!(f, "Settings"),
            IPaneType::AIFact => write!(f, "AI Fact"),
            IPaneType::AIDocument => write!(f, "AI Document"),
            IPaneType::ExecutionProfileEditor => write!(f, "Execution Profile Editor"),
            IPaneType::GetStarted => write!(f, "GetStarted"),
            IPaneType::SshServer => write!(f, "SSH Server"),
            IPaneType::Sftp => write!(f, "SFTP"),
            IPaneType::Observatory => write!(f, "Observatory"),
            IPaneType::Welcome => write!(f, "Welcome"),
            #[cfg(not(target_family = "wasm"))]
            IPaneType::Cockpit => write!(f, "Cockpit"),
            IPaneType::DeferredPlaceholder => write!(f, "Placeholder"),
            IPaneType::Dummy => write!(f, "Dummy"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pane_id() -> PaneId {
        PaneId(IPaneId {
            pane_type: IPaneType::Terminal,
            pane_view_id: EntityId::from_usize(42),
        })
    }

    /// 幂等验收: serde round-trip 恒等 (快照序列化依赖此不变量)。
    #[test]
    fn pane_id_serde_round_trip_is_identity() {
        let id = sample_pane_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: PaneId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        // 二次 round-trip 结果一致 (幂等)。
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json, json2);
    }

    /// Display 面向日志/诊断, 格式漂移会破坏对账 — 锁定。
    #[test]
    fn pane_id_display_is_stable() {
        let id = sample_pane_id();
        assert_eq!(id.to_string(), "Pane Terminal (42)");
        assert_eq!(IPaneType::Cockpit.to_string(), "Cockpit");
        assert_eq!(IPaneType::Sftp.to_string(), "SFTP");
    }

    /// Ord 全序: HashMap key / 排序依赖 (left_rail_status 聚合依赖)。
    #[test]
    fn pane_ids_are_totally_ordered() {
        let a = sample_pane_id();
        let mut b = a;
        let other = PaneId(IPaneId {
            pane_type: IPaneType::Code,
            pane_view_id: EntityId::from_usize(42),
        });
        assert_ne!(a, other);
        assert!(a < other || a > other);
        let _ = &mut b; // Copy 语义可用
    }

    /// Terminal 收窄: Terminal 型可还原 TerminalPaneId, 非 Terminal 返回 None。
    #[test]
    fn terminal_pane_id_narrows_only_terminal() {
        let t = TerminalPaneId(EntityId::from_usize(7));
        let id: PaneId = t.into();
        assert_eq!(id.terminal_pane_id(), Some(t));
        let code = PaneId(IPaneId {
            pane_type: IPaneType::Code,
            pane_view_id: EntityId::from_usize(7),
        });
        assert_eq!(code.terminal_pane_id(), None);
    }
}
