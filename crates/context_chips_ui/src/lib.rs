//! context_chips_ui — context chips 域纯模型层 crate(P-ARC R5,依 R1-R4
//! 同刀切模式)。
//!
//! 自 `app/src/context_chips` 同刀切出的零 app 依赖部分:
//! - [`context_chip`]:ShellCommandGenerator / ShellCommand(serde POD)、
//!   ExternalCommandsAvailability、ChipDisabledReason / ChipAvailability、
//!   ChipFingerprintInput / ChipRuntimePolicy(缓存指纹策略)、RefreshConfig、
//!   Environment(会话环境快照 POD)。ShellType 经 warp_terminal::shell 引入。
//! - [`spacing`]:context chips / UDI 布局常量。
//!
//! 只依赖既有 crate 面(serde / warp_terminal),禁止依赖任何 app 类型
//! (无 `dais::` / `crate::` 反向引用),零 `ObjectStoreModel` 引用。
//!
//! ## 留守 app 的部分(物理依赖约束,R4 留守层同型)
//!
//! - `ChipRuntimeCapabilities`:`from_session*` 读宿主
//!   `terminal::model::session::Session`,且被 workspace/view.rs 以全限定
//!   关联函数语法调用(R2-b WorkflowViewMode 同型裁决);
//! - `GeneratorContext<'a>` / `PromptGenerator` / `ContextChip`:持有宿主
//!   `terminal::model::{Block, BlockMetadata, Session}` 引用与宿主
//!   `ChipValue`(GitLineChanges 依赖 code_review::DiffStats,经
//!   terminal/view/tab_metadata.rs 全限定调用,均未 crate 化);
//! - `ContextChipKind` / `ChipValue` / `ChipResult`(mod.rs):`to_chip()`
//!   接 builtins + 宿主 ContextChip,`default_styles` 接宿主
//!   themes::PromptColors / RendererStyles,且 `to_chip` 被
//!   workspace/view.rs、prompt/editor_modal.rs 域外调用(R2-b 同型);
//! - `builtins` / `current_prompt` / `display` / `display_chip` /
//!   `display_menu` / `logging` / `node_version_popup` / `prompt` /
//!   `prompt_snapshot` / `prompt_type` / `renderer`:terminal::model /
//!   settings / ai / code_review / menu / ui_components 等宿主依赖,
//!   随 terminal 域 crate 化再迁;
//! - `Environment::from_block` / `ChipRuntimePolicy::availability`:参数为
//!   宿主类型,以宿主 ext trait 留守(app 侧 context_chip.rs 的
//!   `EnvironmentHostExt` / `ChipRuntimePolicyHostExt`,R3 PaneDataHost 模式)。
//!
//! ## 宿主契约
//!
//! app 侧 `crate::context_chips::{context_chip::*, spacing::*}` 旧路径经
//! 宿主 mod shim 指回本 crate,域内外调用点零改动;本 crate 无
//! ObjectStoreModel 访问,无需数据桥。

pub mod context_chip;
pub mod spacing;
