use warp_core::ui::appearance::Appearance;
use warpui::{
    platform::WindowStyle, AddSingletonModel, App, SingletonEntity, TypedActionView, ViewHandle,
};

use crate::{
    ai::blocklist::BlocklistAIHistoryModel,
    auth::{AuthManager, AuthStateProvider},
    cloud_object::{
        model::{
            actions::ObjectActions, persistence::ObjectStoreModel, view::ObjectStoreViewModel,
        },
        update_manager::UpdateManager,
        ObjectType, Owner, Space,
    },
    drive::{items::WarpDriveItemId, ObjectTypeAndId},
    network::NetworkStatus,
    notebooks::{NotebookObject, NotebookObjectModel},
    server::ids::{ClientId, SyncId},
    settings_view::keybindings::KeybindingChangedNotifier,
    test_util::settings::initialize_settings_for_tests,
    workspaces::{user_profiles::UserProfiles, user_workspaces::UserWorkspaces},
    Assets,
};

use super::{DriveIndex, DriveIndexAction};

fn initialize_app(app: &mut App) {
    initialize_settings_for_tests(app);

    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(ObjectStoreViewModel::mock);
    app.add_singleton_model(|_| ObjectActions::new(Vec::new()));
    app.add_singleton_model(|_| UserProfiles::new(Vec::new()));
    app.add_singleton_model(|_| KeybindingChangedNotifier::mock());
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    #[cfg(feature = "voice_input")]
    app.add_singleton_model(voice_input::VoiceInput::new);
}

fn create_index(app: &mut App) -> ViewHandle<DriveIndex> {
    let (_, index) = app.add_window(WindowStyle::NotStealFocus, DriveIndex::new);
    index
}

fn create_notebook(app: &mut App) -> SyncId {
    ObjectStoreModel::handle(app).update(app, |object_store_model, ctx| {
        let client_id = ClientId::new();
        let sync_id = SyncId::ClientId(client_id);
        object_store_model.create_object(
            sync_id,
            NotebookObject::new_local(
                NotebookObjectModel::default(),
                Owner::mock_current_user(),
                None,
                client_id,
            ),
            ctx,
        );
        sync_id
    })
}

#[test]
fn test_warp_drive_navigation_states() {
    use crate::drive::index::DriveIndexAction;
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let index = create_index(&mut app);
        let sync_id = create_notebook(&mut app);
        let object_type_and_id: ObjectTypeAndId =
            ObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);

        index.read(&app, |index, _| {
            assert_eq!(index.selected, None, "Expect selected to be None");
            assert_eq!(
                index.focused_index,
                Some(0),
                "Expect focused_index to be initialized"
            );
        });

        index.update(&mut app, |index, ctx| {
            index.handle_action(&DriveIndexAction::OpenObject(object_type_and_id), ctx);
        });

        index.read(&app, |index, _| {
            assert_eq!(
                index.selected,
                Some(WarpDriveItemId::Object(object_type_and_id)),
                "Expect selected to have correct value"
            );
        });
    });
}
