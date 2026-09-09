//! env_vars_ui — 环境变量域 (L4 domain-ui, P-ARC R1 低危子集)。
//!
//! nav 模板: crate 自持模型 + 视图, 对宿主 (app) 只走 Data trait 注入,
//! 禁止依赖任何 app 类型 (无 `dais::` / `crate::` 反向引用)。
//!
//! 本轮范围 (低危子集, 依 env_vars 契约):
//! - G1 数据模型 [`model`]:EnvVar / EnvVarValue / EnvVarCollection /
//!   EnvVarCollectionObject / serialize_variables_for_shell + POD 依赖
//!   EnvVarSecretCommand / ExternalSecret (自 app 上移);
//! - G2 状态机 [`data`]:ActiveEnvVarCollection(Data) 经 [`data::EnvVarData`]
//!   trait 访问对象存储 (宿主 = app 包 ObjectStoreModel/ViewModel);
//!
//! 留 app (未断网, 依契约 G4/G5):manager.rs、env_var_collection_block.rs、
//! view/{menus,secrets,command_dialog impl,env_var_collection}.rs 与 3 个
//! view 测试 — 它们依赖 editor/pane_group/search/menu 等 app 模块。

pub mod data;
pub mod model;

pub use model::{
    EnvVar, EnvVarCollection, EnvVarCollectionObject, EnvVarCollectionObjectModel,
    EnvVarCollectionType, EnvVarSecretCommand, EnvVarValue, ExternalSecret,
    serialize_variables_for_shell,
};
