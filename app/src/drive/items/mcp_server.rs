use super::{WarpDriveItem, WarpDriveItemId};
use crate::{
    ai::mcp::MCPServerObject,
    appearance::Appearance,
    drive::{DriveObjectType, ObjectTypeAndId},
    themes::theme::Fill,
};
use objects::StoredObjectMetadata;
use warpui::{elements::MouseStateHandle, AppContext, Element};

#[derive(Clone)]
pub struct WarpDriveMCPServer {
    id: ObjectTypeAndId,
    mcp_server: MCPServerObject,
}

impl WarpDriveMCPServer {
    pub fn new(id: ObjectTypeAndId, mcp_server: MCPServerObject) -> Self {
        Self { id, mcp_server }
    }
}

impl WarpDriveItem for WarpDriveMCPServer {
    fn display_name(&self) -> Option<String> {
        Some(self.mcp_server.model().string_model.name.clone())
    }
    fn metadata(&self) -> Option<&StoredObjectMetadata> {
        Some(&self.mcp_server.metadata)
    }

    fn object_type(&self) -> Option<DriveObjectType> {
        Some(DriveObjectType::MCPServer)
    }

    fn secondary_icon(&self, _color: Option<Fill>) -> Option<Box<dyn Element>> {
        None
    }


    fn preview(&self, _appearance: &Appearance) -> Option<Box<dyn Element>> {
        // TODO
        None
    }

    fn warp_drive_id(&self) -> WarpDriveItemId {
        WarpDriveItemId::Object(self.id)
    }

    fn sync_status_icon(
        &self,
        _sync_queue_is_dequeueing: bool,
        _hover_state: MouseStateHandle,
        _appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        None
    }

    fn action_summary(&self, _app: &AppContext) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn WarpDriveItem> {
        Box::new(self.clone())
    }
}
