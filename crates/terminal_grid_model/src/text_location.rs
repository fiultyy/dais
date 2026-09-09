//! R14: 自 app/src/ai/blocklist/block.rs 提升的文本定位枚举。
//! 零依赖纯 Copy 数据类型;ai 域(消费方)经 app 侧 re-export shim 旧路径全兼容。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextLocation {
    Output {
        section_index: usize,
        /// Note that this does ***not*** correspond to the frame index of a text frame after layout;
        /// Instead, it represents the line index of the `FormattedTextLine` after markdown parsing.
        line_index: usize,
    },
    Query {
        input_index: usize,
    },
    Action {
        action_index: usize,
        line_index: usize,
    },
}
