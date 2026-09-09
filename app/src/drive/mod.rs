pub mod objects_styling;
pub mod drive_helpers;
pub mod export;
pub mod folders;
pub mod items;
pub mod settings;
pub mod sharing;

use std::cmp::Ordering;
use serde::{Deserialize, Serialize};
use warp_core::user_preferences::GetUserPreferences as _;
use warpui::AppContext;

use crate::objects::{ObjectType, StoredObject, ids::ServerId};
use crate::objects::model::view::{ObjectStoreViewModel, UpdateTimestamp};

type SortByComparator<'a> = dyn FnMut(&&dyn StoredObject, &&dyn StoredObject) -> Ordering + 'a;

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct DaisDriveObjectSettings {
    /// The folder that should be focused in the Dais Drive when the object is opened.
    pub focused_folder_id: Option<ServerId>,
    /// The email of the user to invite to the object, if the object is being opened via the request access flow.
    pub invitee_email: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DaisDriveObjectArgs {
    pub object_type: ObjectType,
    pub server_id: ServerId,
    pub settings: DaisDriveObjectSettings,
}

pub use objects::ObjectTypeAndId;
pub use drive_ui::DriveObjectType;

pub fn should_auto_open_welcome_folder(app: &mut AppContext) -> bool {
    app.private_user_preferences()
        .read_value(settings::HAS_AUTO_OPENED_WELCOME_FOLDER)
        .unwrap_or_default()
        .and_then(|s| serde_json::from_str(&s).ok())
        .map(|has_opened: bool| !has_opened)
        .unwrap_or(true)
}

pub fn write_has_auto_opened_welcome_folder_to_user_defaults(app: &mut AppContext) {
    let _ = app
        .private_user_preferences()
        .write_value(settings::HAS_AUTO_OPENED_WELCOME_FOLDER, true.to_string());
}

/// Enum used for sorting elements in the Dais Drive Index (and potentially other places).
/// In the future it can be used to add other options (like, by name or by author), and exposed to
/// users in the index.
#[derive(
    Default,
    PartialEq,
    Eq,
    Hash,
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Sort order for Dais Drive items.",
    rename_all = "snake_case"
)]
pub enum DriveSortOrder {
    /// Sort by newest revision first in main index, most recently trashed in trash index
    #[default]
    ByTimestamp,
    /// A => Z
    AlphabeticalDescending,
    /// Z => A
    AlphabeticalAscending,
    /// Sort by object type, with folders first
    ByObjectType,
}

impl DriveSortOrder {
    /// Returns the comparator that can be used for sorting items returned by
    /// ObjectStoreModel::objectss_in_space, for example (so more specifically, on the iterator of
    /// type Iterator<Item = &'_ dyn StoredObject>)
    pub fn sort_by<'a>(
        &self,
        object_store_model: &'a ObjectStoreViewModel,
        update_timestamp: UpdateTimestamp,
        app: &'a AppContext,
    ) -> Box<SortByComparator<'a>> {
        match self {
            // Sorts newly-created objects to be at the top of the list
            Self::ByTimestamp => Box::new(
                move |a: &&dyn StoredObject, b: &&dyn StoredObject| -> Ordering {
                    object_store_model
                        .object_sorting_timestamp(*a, update_timestamp, app)
                        .cmp(&object_store_model.object_sorting_timestamp(
                            *b,
                            update_timestamp,
                            app,
                        ))
                        .reverse()
                },
            ),
            Self::AlphabeticalDescending => Box::new(
                move |a: &&dyn StoredObject, b: &&dyn StoredObject| -> Ordering {
                    a.display_name()
                        .to_lowercase()
                        .cmp(&b.display_name().to_lowercase())
                },
            ),
            Self::AlphabeticalAscending => Box::new(
                move |a: &&dyn StoredObject, b: &&dyn StoredObject| -> Ordering {
                    b.display_name()
                        .to_lowercase()
                        .cmp(&a.display_name().to_lowercase())
                },
            ),
            Self::ByObjectType => Box::new(
                move |a: &&dyn StoredObject, b: &&dyn StoredObject| -> Ordering {
                    let order = |obj: &&dyn StoredObject| match obj.object_type() {
                        ObjectType::Folder => 0,
                        ObjectType::GenericStringObject(_) => 1,
                        ObjectType::Notebook => 2,
                    };

                    // First compare by object type ordering, then by display name alphabetically if equal
                    order(a).cmp(&order(b)).then_with(|| {
                        a.display_name()
                            .to_lowercase()
                            .cmp(&b.display_name().to_lowercase())
                    })
                },
            ),
        }
    }
}
