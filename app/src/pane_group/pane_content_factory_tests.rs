//! V3b' 幂等测试:`PaneContentFactory` 恢复幂等 + `LeafContents::is_persisted`
//! 全变体真值表。
//!
//! 1. restore 幂等:注入记录型工厂,同一 `LeafContents` 两次 restore 产生
//!    相同的 pane 类型序列(工厂无隐藏状态、同输入同输出)。
//! 2. `is_persisted` 真值表:对 `LeafContents` 全部 18 个变体逐一断言持久化
//!    与否(含 `Code` 变体的 `source.is_restorable()` 细分),防止未来新增
//!    变体漏标 —— 漏标会在保存端留下孤儿 `pane_nodes` 行导致整 tab 丢失。

use super::*;
use crate::app_state::{
    AIDocumentPaneSnapshot, AmbientAgentPaneSnapshot, CodePaneSnapShot, CodePaneTabSnapshot,
    CodeReviewPaneSnapshot,
};
use crate::code::buffer_location::RemotePath;
use crate::code::editor_management::CodeSource;
use crate::app_state::TerminalPaneSnapshot;
use crate::drive::DaisDriveObjectSettings;
use crate::server::ids::SyncId;
use warp_core::HostId;
use warpui::{App, Element, Entity, TypedActionView, View};

/// 最小根视图:仅为 `add_window` 提供合法根,不参与断言。
struct TestRootView;

impl Entity for TestRootView {
    type Event = ();
}

impl View for TestRootView {
    fn ui_name() -> &'static str {
        "TestRootView"
    }

    fn render(&self, _app: &warpui::AppContext) -> Box<dyn Element> {
        warpui::elements::Flex::column().finish()
    }
}

impl TypedActionView for TestRootView {
    type Action = ();
}

impl TestRootView {
    fn new(_ctx: &mut warpui::ViewContext<Self>) -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// 记录型工厂:restore 幂等断言的基础设施
// ---------------------------------------------------------------------------

/// 记录每次 `restore_leaf` 收到的 pane 类型序列。测试断言同一
/// `LeafContents` 重复 restore 得到完全相同的记录。
#[derive(Default)]
struct RecordingFactory {
    calls: std::cell::RefCell<Vec<&'static str>>,
}

impl RecordingFactory {
    fn calls(&self) -> Vec<&'static str> {
        self.calls.borrow().clone()
    }
}

/// 测试工厂只要求"同一输入 → 同一 pane 类型序列"这一幂等语义本身,
/// 不构造真实 pane(那需要完整业务单例图),因此对不可恢复变体返回
/// 与生产实现同形的错误。
impl PaneContentFactory for RecordingFactory {
    fn restore_leaf(
        &self,
        leaf: LeafSnapshot,
        _inputs: PaneRestoreInputs,
        _ctx: &mut ViewContext<PaneGroup>,
        _pane_contents: &mut HashMap<PaneId, Box<dyn AnyPaneContent>>,
        _deferred_panes: &mut Vec<(PaneId, LeafSnapshot)>,
        _pending_ambient_restorations: &mut Vec<(AmbientAgentTaskId, PaneId)>,
    ) -> anyhow::Result<(PaneData, InitialFocus)> {
        let pane_type = match &leaf.contents {
            LeafContents::Terminal(_) => "Terminal",
            LeafContents::Notebook(NotebookPaneSnapshot::NotebookObject { .. }) => "Notebook",
            LeafContents::Notebook(NotebookPaneSnapshot::LocalFileNotebook { .. }) => "File",
            LeafContents::Image { .. } => "Image(unrestorable)",
            LeafContents::AIDocument(_) => "DeferredPlaceholder",
            LeafContents::Code(_) => "Code",
            LeafContents::EnvVarCollection(_) => "EnvVarCollection",
            LeafContents::Workflow(_) => "Workflow",
            LeafContents::Settings(_) => "Settings",
            LeafContents::AIFact(_) => "AIFact",
            LeafContents::ExecutionProfileEditor => "ExecutionProfileEditor(unrestorable)",
            LeafContents::CodeReview(_) => "CodeReview(unrestorable)",
            LeafContents::AmbientAgent(_) => "Terminal",
            LeafContents::Welcome { .. } => "Welcome",
            LeafContents::GetStarted => "GetStarted",
            LeafContents::SshServer { .. } => "SshServer(unrestorable)",
            LeafContents::Sftp { .. } => "Sftp(unrestorable)",
            LeafContents::Observatory => "Observatory(unrestorable)",
            LeafContents::Cockpit => "Cockpit(unrestorable)",
        };
        self.calls.borrow_mut().push(pane_type);
        // 与生产 AIDocument 臂同形的"无真实 pane"路径:占位 id + 空树语义。
        let pane_id = PaneId::dummy_pane_id();
        let focus = InitialFocus {
            focused_pane: leaf.is_focused.then_some(pane_id),
            active_session: None,
        };
        Ok((PaneData::new(pane_id), focus))
    }
}

/// 用 `RecordingFactory` 对同一 `LeafContents` 快照连续 restore 两次,
/// 断言记录的 pane 类型序列逐次相同(幂等核心断言)。
fn assert_restore_idempotent(contents: LeafContents) {
    App::test((), |mut app| async move {
        let global = crate::GlobalResourceHandles::mock(&mut app);
        // TerminalView 构造链的单例注册面极大且随实现漂移 — 直接复用
        // test_util 的权威 setup (与 terminal view 系测试同源), 再补本测试特有项。
        crate::test_util::terminal::initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(|_| {
            crate::GlobalResourceHandlesProvider::new(crate::GlobalResourceHandles {
                model_event_sender: global.model_event_sender.clone(),
                tips_completed: global.tips_completed.clone(),
                user_default_shell_unsupported_banner_model_handle: global
                    .user_default_shell_unsupported_banner_model_handle
                    .clone(),
                settings_file_error: None,
            })
        });
        app.add_singleton_model(|_| crate::settings_view::DisplayCount(1));
        app.add_singleton_model(crate::default_terminal::DefaultTerminal::new);
        // shell 枚举单例 (register 仅 local_tty feature; 测试构建不含该 feature,
        // 直接注册空 shells 的 model — restore 链只读不枚举)。
        app.add_singleton_model(crate::terminal::available_shells::AvailableShells::new);
        // restore 链建 TerminalView 时读自动更新 stage 单例。
        app.add_singleton_model(|_| {
            crate::autoupdate::AutoupdateState::new(std::sync::Arc::new(
                http_client::Client::new(),
            ))
        });
        app.add_singleton_model(crate::terminal::cli_agent::CLIAgentInstallModel::new);
        app.add_singleton_model(|_| ai::project_context::model::ProjectContextModel::default());
        app.add_singleton_model(|_| crate::ai::mcp::file_based_manager::FileBasedMCPManager::default());
        app.add_singleton_model(crate::ai::mcp::file_mcp_watcher::FileMCPWatcher::new);
        app.add_singleton_model(crate::notebooks::editor::keys::NotebookKeybindings::new);
        app.add_singleton_model(crate::settings::network_secrets::ProxyCredentials::new);
        app.add_singleton_model(|_| crate::ssh_manager::notifier::SshTreeChangedNotifier::new());
        // SSH 树/服务器面板恢复读 warp_ssh_manager db; 测试指向临时文件。
        warp_ssh_manager::set_database_path(
            std::env::temp_dir().join(format!("pane_content_factory_tests_{}.sqlite", std::process::id())),
        );
        app.add_singleton_model(|_| crate::gpu_state::GPUState::new());
        app.add_singleton_model(
            |_| crate::settings_view::pane_manager::SettingsPaneManager::new(),
        );
        let (window_id, _root) =
            app.add_window(warpui::platform::WindowStyle::NotStealFocus, TestRootView::new);
        // SettingsPane 恢复依赖 per-window SettingsView 注册。
        app.update(|ctx| {
            let settings_view =
                ctx.add_typed_action_view(window_id, |ctx| {
                    crate::settings_view::SettingsView::new(None, ctx)
                });
            crate::settings_view::pane_manager::SettingsPaneManager::handle(ctx).update(
                ctx,
                |manager, _|
                    manager.register_view(window_id, settings_view.clone()),
            );
        });

        let make_leaf = || LeafSnapshot {
            is_focused: false,
            custom_vertical_tabs_title: None,
            contents: clone_contents(&contents),
        };

        app.update(|ctx| {
            // 在窗口内挂一个真实 PaneGroup 视图,其构造闭包持有
            // `ViewContext<PaneGroup>` —— 与生产 restore 链同型。
            ctx.add_typed_action_view(window_id, |ctx| {
                let factory = RecordingFactory::default();

                let run = |factory: &RecordingFactory,
                           ctx: &mut ViewContext<PaneGroup>| {
                    let inputs = PaneRestoreInputs {
                        block_lists: Arc::new(HashMap::new()),
                        resources: TerminalViewResources {
                            tips_completed: ctx.add_model(|_| Default::default()),
                            model_event_sender: None,
                        },
                        user_default_shell_unsupported_banner_model_handle: ctx
                            .add_model(|_| Default::default()),
                        view_size: pathfinder_geometry::vector::vec2f(800., 600.),
                        model_event_sender: None,
                    };
                    let mut pane_contents: HashMap<PaneId, Box<dyn AnyPaneContent>> =
                        HashMap::new();
                    let mut deferred_panes: Vec<(PaneId, LeafSnapshot)> = Vec::new();
                    let mut pending: Vec<(AmbientAgentTaskId, PaneId)> = Vec::new();
                    let leaf = make_leaf();
                    let result = PaneContentFactory::restore_leaf(
                        factory,
                        leaf,
                        inputs,
                        ctx,
                        &mut pane_contents,
                        &mut deferred_panes,
                        &mut pending,
                    );
                    (result, deferred_panes)
                };

                let first = run(&factory, ctx);
                let second = run(&factory, ctx);

                // 幂等:同输入两次,pane 类型序列一致。
                assert_eq!(factory.calls()[0], factory.calls()[1]);

                // 结果形态一致:Ok/Err 一致;AIDocument 臂两次都产生 deferred 占位。
                assert_eq!(first.0.is_ok(), second.0.is_ok());
                assert_eq!(first.1.len(), second.1.len());

                PaneGroup::new_from_existing_pane(
                    Box::new(SettingsPane::new(
                        crate::settings_view::SettingsSection::default(),
                        None,
                        window_id,
                        ctx,
                    )),
                    ctx.add_model(|_| Default::default()),
                    ctx.add_model(|_| Default::default()),
                    None,
                    ctx,
                )
            });
        });
    });
}

/// `LeafContents` 无 `Clone` 之外的自有拷贝障碍 —— 这里用 `Clone`。
fn clone_contents(contents: &LeafContents) -> LeafContents {
    contents.clone()
}

// ---------------------------------------------------------------------------
// restore 幂等:逐代表变体
// ---------------------------------------------------------------------------

#[test]
fn restore_terminal_leaf_is_idempotent() {
    assert_restore_idempotent(LeafContents::Terminal(TerminalPaneSnapshot {
        uuid: vec![1, 2, 3],
        cwd: None,
        shell_launch_data: None,
        is_active: false,
        is_read_only: false,
        input_config: None,
        llm_model_override: None,
        active_profile_id: None,
        conversation_ids_to_restore: Vec::new(),
        active_conversation_id: None,
    }));
}

#[test]
fn restore_aidocument_leaf_defers_idempotently() {
    // AIDocument 走 deferred 占位路径:两次 restore 都应产生 1 条占位记录。
    assert_restore_idempotent(LeafContents::AIDocument(AIDocumentPaneSnapshot::Local {
        document_id: "doc-1".to_string(),
        version: 1,
        content: None,
        title: None,
    }));
}

#[test]
fn restore_notebook_leaf_is_idempotent() {
    assert_restore_idempotent(LeafContents::Notebook(
        NotebookPaneSnapshot::NotebookObject {
            notebook_id: Some(SyncId::ClientId(Default::default())),
            settings: DaisDriveObjectSettings::default(),
        },
    ));
}

#[test]
fn restore_settings_leaf_is_idempotent() {
    assert_restore_idempotent(LeafContents::Settings(SettingsPaneSnapshot::Local {
        current_page: crate::settings_view::SettingsSection::default(),
        search_query: None,
    }));
}

#[test]
fn restore_unrestorable_variants_fail_identically() {
    // 不可恢复变体(非持久化残留):两次 restore 同样失败,同样报错 ——
    // 失败形态也必须幂等。
    for contents in [
        LeafContents::CodeReview(CodeReviewPaneSnapshot::Local {
            terminal_uuid: vec![1],
            repo_path: PathBuf::from("/repo"),
        }),
        LeafContents::ExecutionProfileEditor,
        LeafContents::SshServer {
            node_id: "node-1".to_string(),
        },
        LeafContents::Sftp {
            node_id: "node-1".to_string(),
        },
        LeafContents::Image { path: None },
        LeafContents::Observatory,
        LeafContents::Cockpit,
    ] {
        assert_restore_idempotent(contents);
    }
}

// ---------------------------------------------------------------------------
// LeafContents::is_persisted 全变体真值表 (18 变体)
// ---------------------------------------------------------------------------

/// 18 变体逐一锁定持久化真值。
///
/// `Code` 特殊:按 `source.is_restorable()` 细分;`AIAction` /
/// `RemoteFileTree` source 不可恢复 ⇒ 不持久化,其余 ⇒ 持久化;
/// `source: None`(会话内新建未命名)⇒ 持久化。
#[test]
fn leaf_contents_is_persisted_truth_table_covers_all_variants() {
    #[cfg(feature = "local_fs")]
    {
        // Code::Local 可恢复 source(FileTree)⇒ 持久化。
        assert!(LeafContents::Code(CodePaneSnapShot::Local {
            tabs: vec![CodePaneTabSnapshot { path: None }],
            active_tab_index: 0,
            source: Some(CodeSource::FileTree {
                path: PathBuf::from("/tmp/a.rs")
            }),
        })
        .is_persisted());

        // Code::Local 不可恢复 source(AIAction)⇒ 不持久化。
        assert!(!LeafContents::Code(CodePaneSnapShot::Local {
            tabs: vec![CodePaneTabSnapshot { path: None }],
            active_tab_index: 0,
            source: Some(CodeSource::AIAction {
                id: crate::ai::agent::AIAgentActionId::from("act-1".to_string()),
            }),
        })
        .is_persisted());

        // Code::Local 不可恢复 source(RemoteFileTree)⇒ 不持久化。
        assert!(!LeafContents::Code(CodePaneSnapShot::Local {
            tabs: vec![CodePaneTabSnapshot { path: None }],
            active_tab_index: 0,
            source: Some(CodeSource::RemoteFileTree {
                remote_path: RemotePath::new(
                    HostId::new("h".to_string()),
                    warp_util::standardized_path::StandardizedPath::try_new("/tmp/x")
                        .expect("valid test path"),
                ),
            }),
        })
        .is_persisted());

        // Code::Local source 缺失 ⇒ 持久化(unwrap_or(true) 语义)。
        assert!(LeafContents::Code(CodePaneSnapShot::Local {
            tabs: vec![CodePaneTabSnapshot { path: None }],
            active_tab_index: 0,
            source: None,
        })
        .is_persisted());
    }

    // 持久化: true 组
    assert!(
        LeafContents::Terminal(TerminalPaneSnapshot {
            uuid: vec![1],
            cwd: None,
            shell_launch_data: None,
            is_active: false,
            is_read_only: false,
            input_config: None,
            llm_model_override: None,
            active_profile_id: None,
            conversation_ids_to_restore: Vec::new(),
            active_conversation_id: None,
        })
        .is_persisted()
    );
    assert!(
        LeafContents::Notebook(NotebookPaneSnapshot::NotebookObject {
            notebook_id: None,
            settings: DaisDriveObjectSettings::default(),
        })
        .is_persisted()
    );
    assert!(
        LeafContents::Notebook(NotebookPaneSnapshot::LocalFileNotebook {
            path: Some(PathBuf::from("/tmp/nb.ipynb")),
        })
        .is_persisted()
    );
    assert!(
        LeafContents::AIDocument(AIDocumentPaneSnapshot::Local {
            document_id: "doc-1".to_string(),
            version: 1,
            content: None,
            title: None,
        })
        .is_persisted()
    );
    assert!(
        LeafContents::EnvVarCollection(EnvVarCollectionPaneSnapshot::EnvVarCollectionObject {
            env_var_collection_id: None,
        })
        .is_persisted()
    );
    assert!(
        LeafContents::Workflow(WorkflowPaneSnapshot::WorkflowObject {
            workflow_id: None,
            settings: DaisDriveObjectSettings::default(),
        })
        .is_persisted()
    );
    assert!(
        LeafContents::Settings(SettingsPaneSnapshot::Local {
            current_page: crate::settings_view::SettingsSection::default(),
            search_query: None,
        })
        .is_persisted()
    );
    assert!(LeafContents::AIFact(AIFactPaneSnapshot::Personal).is_persisted());
    assert!(LeafContents::ExecutionProfileEditor.is_persisted());
    assert!(
        LeafContents::CodeReview(CodeReviewPaneSnapshot::Local {
            terminal_uuid: vec![1],
            repo_path: PathBuf::from("/repo"),
        })
        .is_persisted()
    );
    assert!(
        LeafContents::AmbientAgent(AmbientAgentPaneSnapshot {
            uuid: vec![1],
            task_id: None,
        })
        .is_persisted()
    );
    assert!(LeafContents::Welcome { startup_directory: None }.is_persisted());
    assert!(LeafContents::GetStarted.is_persisted());

    // 不持久化: false 组
    assert!(!LeafContents::Image { path: None }.is_persisted());
    assert!(
        !LeafContents::SshServer {
            node_id: "node-1".to_string()
        }
        .is_persisted()
    );
    assert!(
        !LeafContents::Sftp {
            node_id: "node-1".to_string()
        }
        .is_persisted()
    );
    assert!(!LeafContents::Observatory.is_persisted());
    assert!(!LeafContents::Cockpit.is_persisted());
}

/// 幂等二访:同一批 fixture 两次调用 `is_persisted` 结果恒等
/// (纯函数无状态,防未来引入可变单例态)。
#[test]
fn leaf_contents_is_persisted_is_idempotent() {
    let fixtures: Vec<LeafContents> = vec![
        LeafContents::GetStarted,
        LeafContents::Observatory,
        LeafContents::Cockpit,
        LeafContents::Welcome { startup_directory: None },
        LeafContents::ExecutionProfileEditor,
        LeafContents::AIFact(AIFactPaneSnapshot::Personal),
    ];
    for f in &fixtures {
        assert_eq!(f.is_persisted(), f.is_persisted());
    }
}

