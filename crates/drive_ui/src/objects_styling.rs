use pathfinder_color::ColorU;
use warp_core::ui::{
    appearance::Appearance,
    color::{contrast::MinimumAllowedContrast, ContrastingColor},
    theme::Fill,
    theme::color::internal_colors::text_sub,
};

use crate::DriveObjectType;

pub fn warp_drive_icon_color(
    appearance: &Appearance,
    objects_type: DriveObjectType,
) -> ColorU {
    match objects_type {
        DriveObjectType::Workflow => {
            let color: Fill = appearance.theme().terminal_colors().normal.red.into();
            color
                .on_background(
                    appearance.theme().surface_1(),
                    MinimumAllowedContrast::NonText,
                )
                .into()
        }
        DriveObjectType::Notebook { .. } => {
            let color: Fill = appearance.theme().terminal_colors().normal.blue.into();
            color
                .on_background(
                    appearance.theme().surface_1(),
                    MinimumAllowedContrast::NonText,
                )
                .into()
        }
        DriveObjectType::EnvVarCollection => {
            let color: Fill = appearance.theme().terminal_colors().normal.magenta.into();
            color
                .on_background(
                    appearance.theme().surface_1(),
                    MinimumAllowedContrast::NonText,
                )
                .into()
        }
        DriveObjectType::Folder => {
            // Match File Tree styling - use text_sub color
            text_sub(appearance.theme(), appearance.theme().background())
        }
        DriveObjectType::AIFactCollection
        | DriveObjectType::AIFact
        | DriveObjectType::MCPServer
        | DriveObjectType::MCPServerCollection
        | DriveObjectType::AgentModeWorkflow => appearance
            .theme()
            .main_text_color(appearance.theme().background())
            .into(),
    }
}
