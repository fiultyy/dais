//! V3b' LeafContents 不透明化:pane 恢复工厂。
//!
//! [`PaneContentFactory`] 把 `restore_pane_leaf` 的 `LeafContents -> pane`
//! 分派体从 `PaneGroup` 抽出:恢复链只依赖工厂 trait,骨架 crate
//! `pane_tree` 对 app 业务类型保持零感知。业务单例
//! (`RestoredAgentConversations` / `AIExecutionProfilesModel` /
//! `LLMPreferences` / `AvailableShells`) 全部留在生产实现
//! [`AppPaneContentFactory`] 内部;测试可注入记录型工厂做幂等断言
//! (见 `pane_content_factory_tests.rs`)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use pathfinder_geometry::vector::Vector2F;
use warpui::{ModelHandle, SingletonEntity, ViewContext};

use crate::ai::agent_conversations_model::{AgentConversationsModel, ConversationOrTask};
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::llms::LLMId;
use crate::ai::restored_conversations::RestoredAgentConversations;
#[cfg(feature = "local_fs")]
use crate::app_state::CodePaneSnapShot;
use crate::app_state::{
    AIFactPaneSnapshot, EnvVarCollectionPaneSnapshot, LeafContents, LeafSnapshot,
    NotebookPaneSnapshot, PaneUuid, SettingsPaneSnapshot, WorkflowPaneSnapshot,
};
use crate::banner::BannerState;
#[cfg(feature = "local_fs")]
use crate::code::editor_management::CodeSource;
use crate::code::view::CodeView;
use crate::features::FeatureFlag;
use crate::persistence::ModelEvent;
use crate::terminal::available_shells::AvailableShells;
use crate::terminal::view::ConversationRestorationInNewPaneType;
use crate::workspace::WorkspaceAction;

use super::{
    AIFactPane, AnyPaneContent, CodePane, EnvVarCollectionPane, FilePane, GetStartedPane,
    InitialFocus, NotebookPane, PaneContent, PaneData, PaneGroup, PaneId, PaneIdConstruct,
    SettingsPane, TerminalPane, TerminalViewResources, WelcomePane, WorkflowPane,
};

/// restore 链的按值输入包。`Terminal` / `AmbientAgent` 臂会消费
/// `resources` / banner 句柄 / 事件 sender,故整体按值传递。
pub(crate) struct PaneRestoreInputs {
    pub(crate) block_lists: Arc<HashMap<PaneUuid, Vec<SerializedBlockListItem>>>,
    pub(crate) resources: TerminalViewResources,
    pub(crate) user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
    pub(crate) view_size: Vector2F,
    pub(crate) model_event_sender: Option<SyncSender<ModelEvent>>,
}

/// pane 恢复工厂:把一个 leaf 快照分派为具体 pane 内容。
///
/// `restore_pane_leaf` 只负责组装输入、委托本 trait、以及 leaf 级后处理
/// (自定义垂直标签标题回写);变体分派与业务单例访问全部下沉到实现内。
pub(crate) trait PaneContentFactory {
    fn restore_leaf(
        &self,
        leaf: LeafSnapshot,
        inputs: PaneRestoreInputs,
        ctx: &mut ViewContext<PaneGroup>,
        pane_contents: &mut HashMap<PaneId, Box<dyn AnyPaneContent>>,
        deferred_panes: &mut Vec<(PaneId, LeafSnapshot)>,
        pending_ambient_restorations: &mut Vec<(AmbientAgentTaskId, PaneId)>,
    ) -> anyhow::Result<(PaneData, InitialFocus)>;
}

/// The restoration path for an ambient agent pane.
enum AmbientRestoreKind {
    /// Conversation data isn't loaded yet — show a loading pane and
    /// defer the real restoration to the pending-restoration subscription
    /// (which waits for the data to be loaded async).
    PendingRestoration { task_id: AmbientAgentTaskId },
    /// If there's no task ID to restore, we open a fresh ambient-agent pane
    /// (this is a valid state from when a user quits with an empty ambient-agent pane).
    NewCloudConversation,
}

/// 生产实现:零大小结构,按需构造无成本。
pub(crate) struct AppPaneContentFactory;

impl PaneContentFactory for AppPaneContentFactory {
    fn restore_leaf(
        &self,
        leaf: LeafSnapshot,
        inputs: PaneRestoreInputs,
        ctx: &mut ViewContext<PaneGroup>,
        pane_contents: &mut HashMap<PaneId, Box<dyn AnyPaneContent>>,
        deferred_panes: &mut Vec<(PaneId, LeafSnapshot)>,
        pending_ambient_restorations: &mut Vec<(AmbientAgentTaskId, PaneId)>,
    ) -> anyhow::Result<(PaneData, InitialFocus)> {
        let PaneRestoreInputs {
            block_lists,
            resources,
            user_default_shell_unsupported_banner_model_handle,
            view_size,
            model_event_sender,
        } = inputs;
        match leaf.contents {
            LeafContents::AIDocument(_) => {
                // Defer AI document pane restoration until after terminal panes are restored.
                // We do this because the terminal view seeds the AIDocumentModel as part of
                // conversation restoration, and the AIDocumentView requires the data to already
                // exist in the AIDocumentModel. In practice, this will work most of the time
                // because the AIDocumentView is usually in the same tab as the terminal view containing
                // the conversation data.
                // TODO (roland): this is not ideal. If the AIDocumentView is moved to an earlier tab
                // than the terminal view with the data, the data won't exist when the AIDocumentView is restored. Right now
                // the AIDocumentView handles this case and renders with an empty buffer until the data is restored.
                // But if the AIDocumentView is leftover after the terminal view containing the conversation
                // is closed, the data would never be loaded because the conversation is never restored.
                let pane_id = PaneId::deferred_placeholder_pane_id();
                let is_focused = leaf.is_focused;
                deferred_panes.push((pane_id, leaf));
                let focus = InitialFocus {
                    focused_pane: is_focused.then_some(pane_id),
                    active_session: None,
                };
                Ok((PaneData::new(pane_id), focus))
            }
            LeafContents::Terminal(terminal_snapshot) => {
                let uuid = PaneUuid(terminal_snapshot.uuid.clone());
                let block_list = block_lists.get(&uuid);

                let chosen_shell = terminal_snapshot
                    .shell_launch_data
                    .as_ref()
                    .and_then(|shell| {
                        if FeatureFlag::ShellSelector.is_enabled() {
                            AvailableShells::as_ref(ctx).get_from_shell_launch_data(shell)
                        } else {
                            None
                        }
                    });

                let startup_directory = terminal_snapshot
                    .cwd
                    .map(PathBuf::from)
                    .filter(|path| path.is_dir());

                // Filter conversation IDs to only include those that have task messages
                // and are not entirely passive (ignored suggestions).
                // This prevents showing the "Previous session" banner when there's nothing to restore
                // and avoids restoring passive code diffs that the user never acted on.
                let filtered_conversation_ids: Vec<crate::ai::agent::conversation::AIConversationId> =
                    terminal_snapshot
                        .conversation_ids_to_restore
                        .iter()
                        .filter(|&conversation_id| {
                            RestoredAgentConversations::handle(ctx).read(ctx, |store, _| {
                                store
                                    .get_conversation(conversation_id)
                                    .is_some_and(|persisted_conv| {
                                        // Filter conversations that contain no tasks.
                                        if persisted_conv.all_tasks().next().is_none() {
                                            return false;
                                        }

                                        // Filter conversations that are entirely passive.
                                        !persisted_conv.is_entirely_passive()
                                    })
                            })
                        })
                        .copied()
                        .collect();

                let conversation_restoration =
                    vec1::Vec1::try_from_vec(filtered_conversation_ids).ok().map(
                        |conversation_ids| ConversationRestorationInNewPaneType::Startup {
                            conversation_ids,
                            active_conversation_id: terminal_snapshot.active_conversation_id,
                        },
                    );
                let (terminal_view, terminal_manager) = PaneGroup::create_session(
                    startup_directory,
                    HashMap::new(),
                    crate::terminal::shared_session::IsSharedSessionCreator::No,
                    resources,
                    block_list,
                    conversation_restoration,
                    user_default_shell_unsupported_banner_model_handle,
                    view_size,
                    model_event_sender.clone(),
                    chosen_shell,
                    terminal_snapshot.input_config,
                    ctx,
                );

                let terminal_view_id = terminal_view.id();

                let pane_data = TerminalPane::new(
                    uuid.0,
                    terminal_manager,
                    terminal_view,
                    model_event_sender,
                    ctx,
                );

                let terminal_pane_id = pane_data.terminal_pane_id();
                let pane_id = terminal_pane_id.into();
                pane_contents.insert(pane_id, Box::new(pane_data));

                if let Some(llm_override) = &terminal_snapshot.llm_model_override {
                    if let Ok(llm_id) = serde_json::from_str::<LLMId>(llm_override) {
                        log::info!("Selecting base agent model {llm_id} (from terminal snapshot)");
                        crate::ai::llms::LLMPreferences::handle(ctx).update(
                            ctx,
                            |llm_prefs, ctx| {
                                llm_prefs.update_preferred_agent_mode_llm(
                                    &llm_id,
                                    terminal_view_id,
                                    ctx,
                                );
                            },
                        );
                    }
                }

                if let Some(active_profile_sync_id) = &terminal_snapshot.active_profile_id {
                    log::info!(
                        "Attempting to restore active_profile '{active_profile_sync_id}' for terminal {terminal_view_id:?}"
                    );

                    let profiles_model = AIExecutionProfilesModel::as_ref(ctx);

                    if let Some(profile_id) =
                        profiles_model.get_profile_id_by_sync_id(active_profile_sync_id)
                    {
                        AIExecutionProfilesModel::handle(ctx).update(ctx, |profiles_model, ctx| {
                            profiles_model.set_active_profile(terminal_view_id, profile_id, ctx);
                        });
                        log::info!(
                            "Restored active profile {profile_id:?} for terminal {terminal_view_id:?}"
                        );
                    } else {
                        log::warn!(
                            "Failed to restore active profile for terminal {terminal_view_id:?}"
                        );
                    }
                }

                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: terminal_snapshot.is_active.then_some(terminal_pane_id),
                };

                Ok((PaneData::new(pane_id), focus))
            }
            LeafContents::Notebook(snapshot) => {
                let pane: Box<dyn AnyPaneContent + 'static> = match snapshot {
                    NotebookPaneSnapshot::NotebookObject {
                        notebook_id,
                        settings,
                    } => Box::new(NotebookPane::restore(notebook_id, &settings, ctx)?),
                    NotebookPaneSnapshot::LocalFileNotebook { path } => Box::new(FilePane::new(
                        path,
                        None,
                        #[cfg(feature = "local_fs")]
                        None,
                        ctx,
                    )),
                };

                let pane_id = pane.as_pane().id();
                pane_contents.insert(pane_id, pane);
                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };

                Ok((PaneData::new(pane_id), focus))
            }
            #[cfg(feature = "local_fs")]
            LeafContents::Code(snapshot) => {
                let CodePaneSnapShot::Local {
                    tabs,
                    active_tab_index,
                    source,
                } = snapshot;

                let Some(source) = source.filter(|s: &CodeSource| s.is_restorable()) else {
                    return Err(anyhow::anyhow!(
                        "Skipping code pane with non-restorable source"
                    ));
                };

                let code_view = ctx.add_typed_action_view(move |ctx| {
                    CodeView::restore(&tabs, active_tab_index, source, ctx)
                });
                let pane = CodePane::from_view(code_view, ctx);
                let pane_id = pane.id();
                pane_contents.insert(pane_id, Box::new(pane));
                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };
                Ok((PaneData::new(pane_id), focus))
            }
            #[cfg(not(feature = "local_fs"))]
            LeafContents::Code(_) => Err(anyhow::anyhow!(
                "Code pane restoration not supported on this platform"
            )),
            LeafContents::EnvVarCollection(snapshot) => {
                let pane: Box<dyn AnyPaneContent + 'static> = match snapshot {
                    EnvVarCollectionPaneSnapshot::EnvVarCollectionObject {
                        env_var_collection_id,
                    } => Box::new(EnvVarCollectionPane::restore(env_var_collection_id, ctx)?),
                };

                let pane_id = pane.as_pane().id();
                pane_contents.insert(pane_id, pane);
                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };

                Ok((PaneData::new(pane_id), focus))
            }
            LeafContents::Workflow(snapshot) => {
                let pane: Box<dyn AnyPaneContent + 'static> = match snapshot {
                    WorkflowPaneSnapshot::WorkflowObject {
                        workflow_id,
                        settings,
                    } => Box::new(WorkflowPane::restore(workflow_id, settings, ctx)?),
                };

                let pane_id = pane.as_pane().id();
                pane_contents.insert(pane_id, pane);
                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };

                Ok((PaneData::new(pane_id), focus))
            }
            LeafContents::Settings(snapshot) => {
                let pane: Box<dyn AnyPaneContent + 'static> = match snapshot {
                    SettingsPaneSnapshot::Local {
                        current_page,
                        search_query,
                    } => Box::new(SettingsPane::new(
                        current_page,
                        search_query.as_deref(),
                        ctx.window_id(),
                        ctx,
                    )),
                };

                let pane_id = pane.as_pane().id();
                pane_contents.insert(pane_id, pane);
                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };
                Ok((PaneData::new(pane_id), focus))
            }
            #[cfg(not(target_family = "wasm"))]
            LeafContents::Observatory => Err(anyhow::anyhow!(
                "Observatory pane is not persisted and cannot be restored"
            )),
            LeafContents::Cockpit => Err(anyhow::anyhow!(
                "Cockpit pane is not persisted and cannot be restored"
            )),
            LeafContents::AIFact(snapshot) => {
                if !FeatureFlag::AIRules.is_enabled() {
                    return Err(anyhow::anyhow!("AI fact pane not enabled"));
                }
                let pane: Box<dyn AnyPaneContent + 'static> = match snapshot {
                    AIFactPaneSnapshot::Personal => Box::new(AIFactPane::new(ctx)),
                };
                let pane_id = pane.as_pane().id();
                pane_contents.insert(pane_id, pane);
                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };
                Ok((PaneData::new(pane_id), focus))
            }
            LeafContents::AmbientAgent(snapshot) => {
                let task_data = snapshot.task_id.map(|task_id| {
                    let task = AgentConversationsModel::handle(ctx)
                        .update(ctx, |model, _| model.get_or_async_fetch_task_data(&task_id));
                    (task_id, task)
                });

                let restore_kind = match &task_data {
                    Some((_, Some(task))) => {
                        let item = ConversationOrTask::Task(task);
                        match item.get_open_action(None) {
                            Some(WorkspaceAction::OpenAmbientAgentSession { task_id }) => {
                                AmbientRestoreKind::PendingRestoration { task_id }
                            }
                            // Transcript viewer and other non-session actions depend on conversation metadata from
                            // BlocklistAIHistoryModel, which is loaded asynchronously.
                            // Defer to the pending-restoration handler so it can retry once that metadata arrives.
                            _ => task_data
                                .as_ref()
                                .map(|(tid, _)| AmbientRestoreKind::PendingRestoration {
                                    task_id: *tid,
                                })
                                .unwrap_or(AmbientRestoreKind::NewCloudConversation),
                        }
                    }
                    Some((task_id, None)) => {
                        AmbientRestoreKind::PendingRestoration { task_id: *task_id }
                    }
                    None => AmbientRestoreKind::NewCloudConversation,
                };

                let mut pending_task: Option<AmbientAgentTaskId> = None;
                let (terminal_view, terminal_manager) = match restore_kind {
                    AmbientRestoreKind::PendingRestoration { task_id } => {
                        let (view, manager) = PaneGroup::create_loading_terminal_manager_and_view(
                            resources,
                            view_size,
                            ctx.window_id(),
                            ctx,
                        );
                        pending_task = Some(task_id);
                        (view, manager)
                    }
                    AmbientRestoreKind::NewCloudConversation => {
                        PaneGroup::create_ambient_agent_terminal(resources, view_size, ctx)
                    }
                };

                let pane_data = TerminalPane::new(
                    snapshot.uuid,
                    terminal_manager,
                    terminal_view,
                    model_event_sender,
                    ctx,
                );
                let terminal_pane_id = pane_data.terminal_pane_id();
                let pane_id = terminal_pane_id.into();
                pane_contents.insert(pane_id, Box::new(pane_data));

                if let Some(task_id) = pending_task {
                    // Defer restoration to after the task data is loaded.
                    pending_ambient_restorations.push((task_id, pane_id));
                }

                let focus = InitialFocus {
                    focused_pane: leaf.is_focused.then_some(pane_id),
                    active_session: None,
                };
                Ok((PaneData::new(pane_id), focus))
            }
            LeafContents::CodeReview(_) => {
                Err(anyhow::anyhow!("Code review panes are no longer supported"))
            }
            LeafContents::ExecutionProfileEditor => {
                // We don't yet support restoring execution profile editor panes.
                Err(anyhow::anyhow!(
                    "Can't restore execution profile editor panes"
                ))
            }
            LeafContents::SshServer { .. } => {
                // SSH server editor panes are intentionally not restored —
                // they're transient editor surfaces over the persistent
                // `ssh_servers` table. Users reopen via the SSH manager tree
                // in the left panel.
                Err(anyhow::anyhow!(
                    "SSH server pane should not have been persisted, as it cannot be restored"
                ))
            }
            LeafContents::Sftp { .. } => {
                // SFTP 浏览器 pane 不持久化,远端文件系统依赖活跃 SSH 连接,
                // 无法在重启后恢复。
                Err(anyhow::anyhow!(
                    "SFTP pane should not have been persisted, as it cannot be restored"
                ))
            }
            LeafContents::Image { .. } => {
                // Image viewer panes are intentionally not persisted (see
                // `LeafContents::is_persisted`), so this should be unreachable.
                Err(anyhow::anyhow!(
                    "Image pane should not have been persisted, as it is not restorable"
                ))
            }
            LeafContents::GetStarted => {
                if !FeatureFlag::GetStartedTab.is_enabled() {
                    Err(anyhow::anyhow!("GetStarted pane not supported"))
                } else {
                    let pane: Box<dyn AnyPaneContent + 'static> =
                        Box::new(GetStartedPane::new(ctx));
                    let pane_id = pane.as_pane().id();
                    pane_contents.insert(pane_id, pane);
                    let focus = InitialFocus {
                        focused_pane: leaf.is_focused.then_some(pane_id),
                        active_session: None,
                    };
                    Ok((PaneData::new(pane_id), focus))
                }
            }
            LeafContents::Welcome { startup_directory } => {
                if !FeatureFlag::WelcomeTab.is_enabled() {
                    Err(anyhow::anyhow!("Welcome pane not supported"))
                } else {
                    let pane: Box<dyn AnyPaneContent + 'static> =
                        Box::new(WelcomePane::new(startup_directory, ctx));
                    let pane_id = pane.as_pane().id();
                    pane_contents.insert(pane_id, pane);
                    let focus = InitialFocus {
                        focused_pane: leaf.is_focused.then_some(pane_id),
                        active_session: None,
                    };
                    Ok((PaneData::new(pane_id), focus))
                }
            } // Dais Wave 7-3:`EnvironmentManagement` LeafContents arm 随 ambient-agent UI
              // 子系统物理删。
        }
    }
}

#[cfg(test)]
#[path = "pane_content_factory_tests.rs"]
mod tests;
