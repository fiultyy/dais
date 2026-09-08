use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures::channel::oneshot;
use parking_lot::Mutex;
use tempfile::TempDir;
use warp_util::path::ShellFamily;
use warpui::{AddSingletonModel, App, SingletonEntity, WindowId};

use crate::{
    cloud_object::{
        model::persistence::ObjectStoreModel, ObjectType, Space,
        StoredObjectMetadata, StoredObjectPermissions,
    },
    drive::ObjectTypeAndId,
    notebooks::{NotebookId, NotebookObject, NotebookObjectModel},
    server::ids::SyncId,
    workspace::ToastStack,
    workspaces::user_workspaces::UserWorkspaces,
};

use super::{safe_filename, ExportEvent, ExportId, ExportManager};

struct ExportTest {
    target_dir: TempDir,
    pending_exports: Arc<Mutex<HashMap<ExportId, oneshot::Sender<ExportEvent>>>>,
}

impl ExportTest {
    fn new(app: &mut App) -> Self {
        let pending_exports = Arc::new(Mutex::new(
            HashMap::<ExportId, oneshot::Sender<ExportEvent>>::new(),
        ));

        {
            let pending_exports = pending_exports.clone();
            app.update(|ctx| {
                ctx.subscribe_to_model(&ExportManager::handle(ctx), move |_, event, _| {
                    let mut pending_exports = pending_exports.lock();
                    let id = match event {
                        ExportEvent::Canceled(id) => id,
                        ExportEvent::Failed { id, .. } => id,
                        ExportEvent::Completed { id, .. } => id,
                    };
                    if let Some(sender) = pending_exports.remove(id) {
                        let _ = sender.send(event.clone());
                    }
                });
            });
        }

        Self {
            target_dir: TempDir::new().expect("failed to create temporary export directory"),
            pending_exports,
        }
    }

    /// Starts exporting an object into the temporary directory.
    fn start_export(
        &self,
        export_ids: ObjectTypeAndId,
        app: &mut App,
    ) -> (ExportId, oneshot::Receiver<ExportEvent>) {
        let id = ExportId(export_ids, Space::Personal);
        let (tx, rx) = oneshot::channel();
        self.pending_exports.lock().insert(id, tx);

        ExportManager::handle(app).update(app, |export_manager, ctx| {
            let window_id = WindowId::new();
            export_manager.export(window_id, &[export_ids], ctx);
            export_manager.handle_files_picked(
                vec![id],
                Ok(vec![self
                    .target_dir
                    .path()
                    .to_str()
                    .expect("Path must be UTF-8")
                    .to_owned()]),
                ShellFamily::Posix,
                ctx,
            );
            id
        });

        (id, rx)
    }

    /// Get an export path, given the expected name.
    fn path(&self, name: impl AsRef<Path>, space: Option<Space>, app: &App) -> PathBuf {
        if let Some(space) = space {
            let space_name = app.read(|ctx| space.name(ctx));
            self.target_dir.path().join(space_name).join(name)
        } else {
            self.target_dir.path().join(name)
        }
    }
}

fn initialize_app(app: &mut App) {
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(ExportManager::new);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(|_| ToastStack);
}

/// Add a mocked notebook.
fn add_notebook(id: SyncId, title: impl Into<String>, data: impl Into<String>, app: &mut App) {
    ObjectStoreModel::handle(app).update(app, |object_store_model, _ctx| {
        object_store_model.add_object(
            id,
            NotebookObject::new(
                id,
                NotebookObjectModel {
                    title: title.into(),
                    data: data.into(),
                    ai_document_id: None,
                    conversation_id: None,
                },
                StoredObjectMetadata::mock(),
                StoredObjectPermissions::mock_personal(),
            ),
        );
    });
}

#[test]
fn test_export_untitled_notebook() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let notebook_id = SyncId::ServerId(NotebookId::from(456).into());
        add_notebook(notebook_id, "", "This is untitled", &mut app);

        let exporter = ExportTest::new(&mut app);
        let (id, export) = exporter.start_export(
            ObjectTypeAndId::from_id_and_type(notebook_id, ObjectType::Notebook),
            &mut app,
        );
        let expected_path = exporter.path("Untitled.md", None, &app);

        // The export should succeed.
        assert_eq!(
            export.await,
            Ok(ExportEvent::Completed {
                id,
                path: expected_path.clone()
            })
        );

        let contents =
            fs::read_to_string(&expected_path).expect("failed to read exported notebook");
        assert_eq!(&contents, "This is untitled");
    });
}

#[test]
fn test_safe_filename() {
    for (expected_in, expected_out) in [
        (
            "allowed $special %characters",
            "allowed $special %characters",
        ),
        ("warp:drive", "warp_drive"),
        ("a/b/c/d:e", "a_b_c_d_e"),
        ("the\0sneaky\0null", "the_sneaky_null"),
        ("ascii\x03control\x1bchars", "ascii_control_chars"),
    ] {
        assert_eq!(safe_filename(expected_in), expected_out);
    }
}

#[test]
fn test_export_multiple_objects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Create two notebooks
        let notebook_id1 = SyncId::ServerId(NotebookId::from(123).into());
        add_notebook(
            notebook_id1,
            "Test notebook 1",
            "This is test notebook 1",
            &mut app,
        );

        let notebook_id2 = SyncId::ServerId(NotebookId::from(456).into());
        add_notebook(
            notebook_id2,
            "Test notebook 2",
            "This is test notebook 2",
            &mut app,
        );

        let exporter = ExportTest::new(&mut app);

        // Prepare export IDs for all objects
        let export_ids = vec![
            ObjectTypeAndId::from_id_and_type(notebook_id1, ObjectType::Notebook),
            ObjectTypeAndId::from_id_and_type(notebook_id2, ObjectType::Notebook),
        ];

        // Create channels for all exports
        let mut receivers = Vec::new();
        {
            let mut pending_exports = exporter.pending_exports.lock();
            for &id in &export_ids {
                let (tx, rx) = oneshot::channel();
                pending_exports.insert(ExportId(id, Space::Personal), tx);
                receivers.push(rx);
            }
        }

        ExportManager::handle(&app).update(&mut app, |export_manager, ctx| {
            let window_id = WindowId::new();
            export_manager.export(window_id, &export_ids, ctx);

            let all_export_ids = export_ids
                .iter()
                .map(|&id| ExportId(id, Space::Personal))
                .collect::<Vec<_>>();
            export_manager.handle_files_picked(
                all_export_ids,
                Ok(vec![exporter
                    .target_dir
                    .path()
                    .to_str()
                    .expect("Path must be UTF-8")
                    .to_owned()]),
                ShellFamily::Posix,
                ctx,
            );
        });

        // Wait for all exports to complete
        for rx in receivers {
            let result = rx.await;
            assert!(
                matches!(result, Ok(ExportEvent::Completed { .. })),
                "Export failed or was canceled"
            );
        }

        // Verify the contents of each exported file
        let notebook1_path =
            exporter.path("Test notebook 1.md", Some(Space::Personal), &app);
        let notebook1_contents =
            fs::read_to_string(&notebook1_path).expect("Failed to read notebook 1");
        assert!(notebook1_contents.contains("This is test notebook 1"));

        let notebook2_path =
            exporter.path("Test notebook 2.md", Some(Space::Personal), &app);
        let notebook2_contents =
            fs::read_to_string(&notebook2_path).expect("Failed to read notebook 2");
        assert!(notebook2_contents.contains("This is test notebook 2"));

        // Check that all files were created
        assert!(notebook1_path.exists(), "Notebook 1 file does not exist");
        assert!(notebook2_path.exists(), "Notebook 2 file does not exist");
    });
}
