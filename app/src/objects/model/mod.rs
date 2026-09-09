//! P-ARC R1 留守层: 宿主耦合的存储/视图模型(依赖 app 域类型)。
//! 纯数据层已下沉 crates/objects::model。

pub mod persistence;
pub mod view;

#[cfg(test)]
#[path = "model_test.rs"]
mod model_test;
