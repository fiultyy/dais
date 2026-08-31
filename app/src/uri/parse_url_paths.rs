use crate::cloud_object::ObjectType;
use crate::drive::{DaisDriveObjectArgs, DaisDriveObjectSettings};
use crate::server::ids::ServerId;
use crate::ChannelState;
use std::collections::HashMap;
use url::Url;

#[derive(PartialEq, Debug)]
pub enum WarpWebLink {
    Session,
    DriveObject(Box<DaisDriveObjectArgs>),
}

/// Extracts the server id and object type from a (caller validated) Drive link.
/// Intended use is deriving metadata from links such that Dais objects
/// can be opened natively in Dais with no web interaction.
///
/// W3-d8: 原 `cloud_object::extract_server_id_and_object_type_from_warp_drive_link`
/// 随云端链路裁剪删除;uri 域是唯一消费者,故在此本地化实现。
fn extract_server_id_and_object_type_from_warp_drive_link(
    url: &Url,
) -> Option<DaisDriveObjectArgs> {
    let server_id = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|last_segment| last_segment.split('-').next_back())
        .map(|id| id.to_string());

    let object_type = url.path_segments().and_then(|mut segments| segments.nth(1));

    // Parse the object portion of the path segment ({channel}/drive/{object})
    // into an object type
    let object_type = match object_type {
        Some("notebook") => ObjectType::Notebook,
        Some("workflow") => ObjectType::Workflow,
        _ => return None,
    };
    let query_string: HashMap<_, _> = url.query_pairs().collect();
    let focused_folder_id: Option<ServerId> = query_string
        .get("focused_folder_id")
        .and_then(|s| s.to_string().try_into().ok());

    let invitee_email: Option<String> = query_string.get("invitee_email").map(|s| s.to_string());

    Some(DaisDriveObjectArgs {
        object_type,
        server_id: match server_id {
            Some(server_id) => server_id.try_into().ok()?,
            _ => return None,
        },
        settings: DaisDriveObjectSettings {
            focused_folder_id,
            invitee_email,
        },
    })
}

pub fn get_item_data_from_warp_link(url: &Url) -> Option<WarpWebLink> {
    if url.origin() == ChannelState::server_root_domain() {
        url.path_segments().and_then(|mut path_segments| {
            path_segments.next().and_then(|segment| match segment {
                "drive" => extract_server_id_and_object_type_from_warp_drive_link(url)
                    .map(|args| WarpWebLink::DriveObject(Box::new(args))),
                "session" => Some(WarpWebLink::Session),
                _ => None,
            })
        })
    } else {
        None
    }
}
