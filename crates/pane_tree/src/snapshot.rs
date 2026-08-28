//! 快照骨架类型 (v2 自 app/src/app_state.rs 下沉泛型化)。
//!
//! 叶子 payload 由宿主 crate 泛型注入 (app 侧绑定: `pub type PaneNodeSnapshot =
//! pane_tree::snapshot::PaneNodeSnapshot<LeafContents>` 等)。
//! 幂等锁定: serde round-trip byte-equal / `has_horizontal_split` 同输入同输出。

use serde::{Deserialize, Serialize};

/// 分屏方向。水平分屏时新 pane 从左到右加入; 垂直分屏时从上到下加入。
#[derive(Clone, Debug, PartialEq, Eq, Copy, Serialize, Deserialize, Hash)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// Pane 布局权重 (flex 比例)。
#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize, Copy)]
pub struct PaneFlex(pub f32);

/// Pane 树节点快照: 分支 (子树拆分) 或叶子 (宿主 pane 内容 `LeafT`)。
#[derive(Clone, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "LeafSnapshot is significantly larger than BranchSnapshot due to nested snapshot types."
)]
pub enum PaneNodeSnapshot<LeafT> {
    Branch(BranchSnapshot<LeafT>),
    Leaf(LeafSnapshot<LeafT>),
}

impl<LeafT: Clone + PartialEq> PaneNodeSnapshot<LeafT> {
    /// 本子树是否存在水平拆分 (同输入恒同输出)。
    pub fn has_horizontal_split(&self) -> bool {
        match self {
            PaneNodeSnapshot::Leaf(_) => false,
            PaneNodeSnapshot::Branch(BranchSnapshot {
                direction,
                children,
            }) => {
                let self_has_split =
                    *direction == SplitDirection::Horizontal && children.len() > 1;
                self_has_split
                    || children
                        .iter()
                        .any(|(_, child)| child.has_horizontal_split())
            }
        }
    }
}

/// 分支快照: 拆分方向 + 带权重子节点。
#[derive(Clone, Debug, PartialEq)]
pub struct BranchSnapshot<LeafT> {
    pub direction: SplitDirection,
    pub children: Vec<(PaneFlex, PaneNodeSnapshot<LeafT>)>,
}

/// 叶子快照: 焦点态 + 垂直标签自定义标题 + 宿主 payload `LeafT`。
#[derive(Clone, Debug, PartialEq)]
pub struct LeafSnapshot<LeafT> {
    pub is_focused: bool,
    pub custom_vertical_tabs_title: Option<String>,
    pub contents: LeafT,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// serde round-trip byte-equal: 序列化→反序列化→再序列化, 字节恒等且值恒等。
    #[test]
    fn split_direction_serde_roundtrip_byte_equal() {
        for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
            let bytes = serde_json::to_vec(&direction).unwrap();
            let restored: SplitDirection = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(restored, direction);
            let bytes_again = serde_json::to_vec(&restored).unwrap();
            assert_eq!(bytes, bytes_again);
        }
    }

    /// serde round-trip byte-equal: 序列化→反序列化→再序列化, 字节恒等且值恒等。
    #[test]
    fn pane_flex_serde_roundtrip_byte_equal() {
        for flex in [
            PaneFlex(0.0),
            PaneFlex(0.5),
            PaneFlex(1.0),
            PaneFlex(2.5),
            PaneFlex(-2.5),
        ] {
            let bytes = serde_json::to_vec(&flex).unwrap();
            let restored: PaneFlex = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(restored, flex);
            let bytes_again = serde_json::to_vec(&restored).unwrap();
            assert_eq!(bytes, bytes_again);
        }
    }

    /// 小树构造 + `has_horizontal_split` 幂等: 同输入两次调用同输出,
    /// 且 Clone 后结论不变。
    #[test]
    fn snapshot_tree_has_horizontal_split_idempotent() {
        let leaf = || {
            PaneNodeSnapshot::Leaf(LeafSnapshot {
                is_focused: false,
                custom_vertical_tabs_title: None,
                contents: (),
            })
        };
        let vertical = PaneNodeSnapshot::Branch(BranchSnapshot {
            direction: SplitDirection::Vertical,
            children: vec![(PaneFlex(1.0), leaf()), (PaneFlex(1.0), leaf())],
        });
        let horizontal = PaneNodeSnapshot::Branch(BranchSnapshot {
            direction: SplitDirection::Horizontal,
            children: vec![(PaneFlex(1.0), leaf()), (PaneFlex(1.0), vertical)],
        });

        // 叶子恒无水平拆分。
        assert!(!leaf().has_horizontal_split());
        // 同输入两次调用同输出 (幂等)。
        let first = horizontal.has_horizontal_split();
        let second = horizontal.has_horizontal_split();
        assert!(first);
        assert_eq!(first, second);
        // Clone 后结论不变 (Clone+PartialEq 语义下的同输入同输出)。
        assert_eq!(horizontal.clone(), horizontal);
        assert_eq!(horizontal.clone().has_horizontal_split(), first);
    }
}
