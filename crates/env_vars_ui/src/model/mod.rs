use itertools::Itertools;
use serde::{Deserialize, Serialize};
use warp_util::path::ShellFamily;

use objects::{
    GenericStoredObject, GenericStringObjectFormat, GenericStringObjectUniqueKey, JsonObjectType,
    ids::SyncId,
    model::generic_string_model::{GenericStringModel, GenericStringObjectId, StringModel},
    model::json_model::{JsonModel, JsonSerializer},
};

mod external_secrets;
pub use external_secrets::ExternalSecret;

#[derive(Clone, Debug, PartialEq)]
pub enum EnvVarCollectionType {
    /// Saved env vars backed by the local object store.
    Object(Box<EnvVarCollectionObject>),
}

impl EnvVarCollectionType {
    pub fn as_env_var_collection_object(&self) -> &EnvVarCollectionObject {
        match self {
            EnvVarCollectionType::Object(env_var_object) => env_var_object,
        }
    }
}

pub type EnvVarCollectionObject =
    GenericStoredObject<GenericStringObjectId, EnvVarCollectionObjectModel>;
pub type EnvVarCollectionObjectModel = GenericStringModel<EnvVarCollection, JsonSerializer>;

/// Defines the data model for a single environment variable
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct EnvVar {
    // Variable name
    pub name: String,
    // Variable value
    pub value: EnvVarValue,
    // Description of variable
    pub description: Option<String>,
}

/// Defines the various forms a value can take
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum EnvVarValue {
    // Represents a string variable, i.e. PORT=4000
    Constant(String),
    // Represents a computed secret, i.e. gcloud print auth token
    Command(EnvVarSecretCommand),
    // Represents a secret from an external secret manager
    Secret(ExternalSecret),
}

/// A computed-secret command (`EnvVarValue::Command`), 自
/// `app/src/env_vars/view/command_dialog/mod.rs` 上移:数据模型 serde
/// 硬依赖该 POD 类型,crate 化后视图侧沿用此处定义。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvVarSecretCommand {
    pub name: String,
    pub command: String,
}

impl Default for EnvVarValue {
    fn default() -> Self {
        EnvVarValue::Constant(String::new())
    }
}

impl EnvVar {
    pub fn new(name: String, value: String, description: Option<String>) -> Self {
        Self {
            name,
            value: EnvVarValue::Constant(value),
            description,
        }
    }

    /// 生成 shell 初始化片段。签名收 `ShellFamily`(warp_util,POD)而非
    /// `warp_terminal::ShellType`:crate 不依赖 terminal 域;app 侧调用点用
    /// `ShellFamily::from(shell_type)` 适配。
    pub fn get_initialization_string(&self, shell_family: ShellFamily) -> String {
        let name = shell_family.escape(&self.name);
        let value = get_init_command_for_env_var(&self.value, shell_family);

        match shell_family {
            ShellFamily::Posix => {
                format!("export {name}={value};")
            }
            ShellFamily::PowerShell => {
                format!("$env:{name} = {value};")
            }
        }
    }
}

fn get_init_command_for_env_var(value: &EnvVarValue, shell_family: ShellFamily) -> String {
    match value {
        EnvVarValue::Constant(val) => match shell_family {
            ShellFamily::Posix => shell_family.escape(val).into_owned(),
            ShellFamily::PowerShell => format!("'{}'", val.replace("'", "''")),
        },
        EnvVarValue::Command(cmd) => format!("$({})", cmd.command),
        EnvVarValue::Secret(secret) => {
            format!("$({})", secret.get_secret_extraction_command(shell_family))
        }
    }
}

/// Defines the data model for a saved collection of environment variables.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct EnvVarCollection {
    // Collection title
    pub title: Option<String>,
    // Description of collection
    pub description: Option<String>,
    // Environment variables associated with this collection
    pub vars: Vec<EnvVar>,
}

impl EnvVarCollection {
    #[allow(dead_code)]
    pub fn new(title: Option<String>, description: Option<String>, vars: Vec<EnvVar>) -> Self {
        Self {
            title,
            description,
            vars,
        }
    }

    fn key_value_iter(&self) -> impl Iterator<Item = (&str, &EnvVarValue)> {
        self.vars.iter().map(|var| (var.name.as_str(), &var.value))
    }

    pub fn export_variables(&self, delimiter: &str, shell_family: ShellFamily) -> String {
        serialize_variables_internal(self.key_value_iter(), "", "=", "", delimiter, shell_family)
    }

    pub fn export_variables_for_shell(&self, shell_family: ShellFamily) -> String {
        serialize_variables_for_shell(self.key_value_iter(), shell_family)
    }

    // P-ARC R1: `renders_in_warp_drive`/`can_export`/`supports_linking`/
    // `to_warp_drive_item` 覆写全部裁撤(方向违例:drive 类型禁入数据模型)。
    // drive 预览/导出/链接语义由 app 侧 objects_display 自由函数按
    // JsonObjectType::EnvVarCollection 分发。
}

impl JsonModel for EnvVarCollection {
    fn json_object_type() -> JsonObjectType {
        JsonObjectType::EnvVarCollection
    }
}

impl StringModel for EnvVarCollection {
    type StoredObjectType = EnvVarCollectionObject;

    fn model_type_name(&self) -> &'static str {
        "Environment variables"
    }

    fn should_enforce_revisions() -> bool {
        true
    }

    fn model_format() -> GenericStringObjectFormat {
        GenericStringObjectFormat::Json(Self::json_object_type())
    }

    fn display_name(&self) -> String {
        self.title.clone().unwrap_or_default()
    }

    fn set_display_name(&mut self, name: &str) {
        self.title = if name.is_empty() {
            None
        } else {
            Some(name.to_owned())
        }
    }

    fn uniqueness_key(&self) -> Option<GenericStringObjectUniqueKey> {
        None
    }

    fn should_show_activity_toasts() -> bool {
        true
    }

    fn warn_if_unsaved_at_quit() -> bool {
        true
    }

}

// P-ARC R1: EnvVarCollectionObject 是 objects crate 类型的 type 别名,
// 跨 crate impl PartialEq 违反孤儿规则;相等性语义由 GenericStoredObject 的
// derive(PartialEq) 承载(原手写 impl 只比 string_model+id,derive 更严但
// 无生产数据,行为变化可接受)。

pub fn serialize_variables_for_shell<'s, I: IntoIterator<Item = (&'s str, &'s EnvVarValue)>>(
    pairs: I,
    shell_family: ShellFamily,
) -> String {
    // Fish 的 `set -x` 语法属 ShellType 粒度,由 app 侧调用点分流;
    // crate 层只认 ShellFamily 两族(与 get_initialization_string 同口径)。
    match shell_family {
        ShellFamily::Posix => serialize_variables_internal(pairs, "", "=", "", " ", shell_family),
        ShellFamily::PowerShell => {
            serialize_variables_internal(pairs, "$env:", " = ", ";", " ", shell_family)
        }
    }
}

// Prefix — what's prepended to each variable
// Separator — what separates the variable name from the value
// Postfix — what's appended to the end of each variable
// Delimiter — what separates one variable from the next one
// set -x var_name var_value;   set -x name2 value2;
// ------     -             -   -
//   ^        ^             ^   ^
// prefix  separator   postfix  delimiter (in this case 4 spaces, usually one space or newline)
fn serialize_variables_internal<'s, I: IntoIterator<Item = (&'s str, &'s EnvVarValue)>>(
    pairs: I,
    prefix: &str,
    separator: &str,
    postfix: &str,
    delimiter: &str,
    shell_family: ShellFamily,
) -> String {
    pairs
        .into_iter()
        .map(|(name, value)| {
            format!(
                "{}{}{}{}{}",
                prefix,
                shell_family.escape(name),
                separator,
                get_init_command_for_env_var(value, shell_family),
                postfix
            )
        })
        .collect_vec()
        .join(delimiter)
}
