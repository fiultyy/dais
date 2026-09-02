//! editor 纯类型层(line / line_iterator / scroll)。
//!
//! 视图/模型主体(view / element / model / comments / comment_editor /
//! embedded_comment / nav_bar / find / goto_line / diff)因 `crate::t!` i18n、
//! `crate::appearance`、`crate::code_review` 反向锚等宿主依赖留守
//! `app/src/code/editor`(P-ARC R6 留守边界,详见 crate lib.rs 头注释)。

pub mod line;
pub mod line_iterator;
pub mod scroll;
