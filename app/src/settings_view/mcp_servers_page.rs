use std::collections::HashMap;
use uuid::Uuid;
use warpui::{
    elements::{ChildView, Container},
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use crate::{
    ai::mcp::{
        gallery::MCPGalleryManager, templatable_installation::VariableValue, FileBasedMCPManager,
        TemplatableMCPServer, TemplatableMCPServerInstallation, TemplatableMCPServerManager,
    },
    appearance::Appearance,
    objects::update_manager::InitiatedBy,
    objects::Space,
    settings_view::{
        mcp_servers::{
            edit_page::{MCPServersEditPageView, MCPServersEditPageViewEvent},
            list_page::{MCPServersListPageView, MCPServersListPageViewEvent},
            style, ServerCardItemId,
        },
        settings_page::{MatchData, PageType, SettingsPageMeta, SettingsWidget},
        SettingsSection,
    },
    view_components::DismissibleToast,
    workspace::ToastStack,
};

/// Describes where an MCP install request originated.
///
/// W3 域裁切:installation modal 已物理删。In-app 请求视为用户已确认,
/// 直接安装;Deeplink 请求失去确认入口,一律拒绝。
/// (原语义见 `specs/GH686/product.md`。)
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InstallOrigin {
    /// Triggered by a user gesture inside Dais (gallery card click,
    /// reinstall button, programmatic in-app flows, etc.).
    InApp,
    /// Triggered by a `warp://settings/mcp?autoinstall=...` deeplink; rejected
    /// because the confirmation modal no longer exists.
    Deeplink,
}

#[derive(Debug, Default, Copy, Clone)]
pub enum MCPServersSettingsPage {
    #[default]
    List,
    Edit {
        item_id: Option<ServerCardItemId>,
    },
}


pub struct MCPServersSettingsPageView {
    page: PageType<Self>,
    current_page: MCPServersSettingsPage,
    list_view: ViewHandle<MCPServersListPageView>,
    edit_view: ViewHandle<MCPServersEditPageView>,
}

impl MCPServersSettingsPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let list_view = ctx.add_typed_action_view(MCPServersListPageView::new);
        ctx.subscribe_to_view(&list_view, |me, _, event, ctx| {
            me.handle_list_view_event(event, ctx);
        });

        let edit_view = ctx.add_typed_action_view(MCPServersEditPageView::new);
        ctx.subscribe_to_view(&edit_view, |me, _, event, ctx| {
            me.handle_edit_view_event(event, ctx);
        });


        Self {
            page: PageType::new_monolith(
                MCPServersSettingsWidget::default(),
                Some(Box::leak(
                    crate::t!("settings-mcp-page-title").into_boxed_str(),
                )),
                true,
            ),
            current_page: MCPServersSettingsPage::default(),
            list_view,
            edit_view,
        }
    }

    pub fn update_page(&mut self, page: MCPServersSettingsPage, ctx: &mut ViewContext<Self>) {
        self.current_page = page;
        if let MCPServersSettingsPage::Edit { item_id } = page {
            self.edit_view.update(ctx, |edit_view, ctx| {
                edit_view.set_mcp_server(item_id, ctx);
            });
        }
        self.focus(ctx);
        ctx.notify();
    }

    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        match self.current_page {
            MCPServersSettingsPage::List => ctx.focus(&self.list_view),
            MCPServersSettingsPage::Edit { .. } => ctx.focus(&self.edit_view),
        }
    }

    fn add_toast(&mut self, message: &str, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(
                DismissibleToast::default(message.to_string()),
                window_id,
                ctx,
            );
        });
    }

    fn handle_log_out(
        &mut self,
        item_id: ServerCardItemId,
        server_name: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let message = match server_name {
            Some(name) => crate::t!("settings-mcp-logout-success-named", name = name),
            None => crate::t!("settings-mcp-logout-success"),
        };
        match item_id {
            ServerCardItemId::TemplatableMCP(_) => {
                log::error!("Logging out is not supported for template MCP servers.");
            }
            ServerCardItemId::TemplatableMCPInstallation(uuid) => {
                TemplatableMCPServerManager::handle(ctx).update(ctx, |manager, ctx| {
                    manager.delete_credentials_from_secure_storage(uuid, ctx);
                    manager.shutdown_server(uuid, ctx);
                });
                self.add_toast(&message, ctx);
            }
            ServerCardItemId::GalleryMCP(_) => {
                log::error!("Logging out is not supported for gallery MCP servers.");
            }
            ServerCardItemId::FileBasedMCP(uuid) => {
                if let Some(installation) =
                    FileBasedMCPManager::as_ref(ctx).get_installation_by_uuid(uuid)
                {
                    if let Some(hash) = installation.hash() {
                        TemplatableMCPServerManager::handle(ctx).update(ctx, |manager, ctx| {
                            manager.shutdown_server(uuid, ctx);
                            manager.purge_file_based_server_credentials(&vec![hash], ctx);
                        });
                    }
                }
                self.add_toast(&message, ctx);
            }
        }
    }

    /// Installs a templatable MCP server directly.
    ///
    /// W3 域裁切:installation modal 已随 installation_modal.rs 一同物理删。
    /// In-app 请求(gallery 卡片点击 / reinstall 按钮)视为用户已确认,直接安装;
    /// Deeplink 请求因失去显式确认入口而一律拒绝,绝不静默安装
    /// (原 GH686 不变量:deeplink 必须经用户确认才能安装/拉起)。
    fn start_server_installation(
        &mut self,
        templatable_mcp_server: TemplatableMCPServer,
        instructions_in_markdown: Option<String>,
        origin: InstallOrigin,
        ctx: &mut ViewContext<Self>,
    ) {
        let _ = instructions_in_markdown;
        match origin {
            InstallOrigin::InApp => {
                self.process_server_installation(&templatable_mcp_server, HashMap::new(), ctx);
            }
            InstallOrigin::Deeplink => {
                log::warn!(
                    "Rejecting MCP install for '{}' from deeplink: installation confirmation modal was removed",
                    templatable_mcp_server.name
                );
                self.add_error_toast(
                    crate::t!(
                        "settings-mcp-install-from-link-failed",
                        name = templatable_mcp_server.name
                    ),
                    ctx,
                );
            }
        }
        ctx.notify();
    }

    fn process_server_installation(
        &mut self,
        templatable_mcp_server: &TemplatableMCPServer,
        variable_values: HashMap<String, VariableValue>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<TemplatableMCPServerInstallation> {
        TemplatableMCPServerManager::handle(ctx).update(ctx, |templatable_manager, ctx| {
            if templatable_manager
                .get_server_object(templatable_mcp_server.uuid, ctx)
                .is_none()
            {
                templatable_manager.create_templatable_mcp_server(
                    templatable_mcp_server.clone(),
                    Space::Personal,
                    InitiatedBy::User,
                    ctx,
                );
            }

            let installation = templatable_manager.install_from_template(
                templatable_mcp_server.clone(),
                variable_values.clone(),
                true,
                ctx,
            );
            ctx.notify();
            installation
        })
    }

    pub fn reinstall_server(&mut self, installation_uuid: Uuid, ctx: &mut ViewContext<Self>) {
        let template_uuid =
            TemplatableMCPServerManager::as_ref(ctx).get_template_uuid(installation_uuid);
        if let Some(template_uuid) = template_uuid {
            let templatable_mcp_server =
                TemplatableMCPServerManager::as_ref(ctx).get_templatable_mcp_server(template_uuid);

            if let Some(templatable_mcp_server) = templatable_mcp_server {
                // Reinstall is always an in-app action triggered from the edit page
                // reinstall button; it must not pick up the deeplink confirmation gating.
                self.start_server_installation(
                    templatable_mcp_server.clone(),
                    None,
                    InstallOrigin::InApp,
                    ctx,
                );
            }
        }
    }

    /// Emits an error toast in the current window.
    fn add_error_toast(&mut self, message: String, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(DismissibleToast::error(message), window_id, ctx);
        });
    }

    /// Auto-installs an MCP server from the gallery.
    ///
    /// This is the single sink for `warp://settings/mcp?autoinstall=<title>`
    /// deeplinks; callers must therefore treat the `autoinstall_param` as
    /// untrusted input. The `autoinstall_param` is matched case-insensitively
    /// against gallery titles.
    ///
    /// W3 域裁切:installation modal 已物理删,deeplink 请求失去显式确认入口,
    /// 由 `start_server_installation` 统一拒绝,绝不静默安装。
    pub fn autoinstall_from_gallery(
        &mut self,
        autoinstall_param: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        log::info!("Received MCP deeplink autoinstall for value '{autoinstall_param}'");

        let autoinstall_lower = autoinstall_param.to_lowercase();
        let gallery_server = MCPGalleryManager::as_ref(ctx)
            .get_gallery()
            .into_iter()
            .find(|item| item.title().to_lowercase() == autoinstall_lower);
        let Some(gallery_server) = gallery_server else {
            log::warn!(
                "Unrecognized autoinstall value '{autoinstall_param}': no matching gallery item found"
            );
            self.add_error_toast(
                crate::t!("settings-mcp-unknown-server", name = autoinstall_param),
                ctx,
            );
            return;
        };

        // Skip if this gallery item is already installed.
        let gallery_uuid = gallery_server.uuid();
        let already_installed = TemplatableMCPServerManager::as_ref(ctx)
            .get_installed_templatable_servers()
            .values()
            .any(|installation| installation.gallery_uuid() == Some(gallery_uuid));
        if already_installed {
            log::info!(
                "Gallery MCP server '{}' is already installed, skipping autoinstall",
                gallery_server.title()
            );
            return;
        }

        let instructions = gallery_server.instructions_in_markdown().cloned();
        let gallery_title = gallery_server.title().to_string();
        let Ok(templatable_mcp_server) = TemplatableMCPServer::try_from(gallery_server) else {
            log::warn!(
                "Failed to convert gallery item '{autoinstall_param}' to TemplatableMCPServer"
            );
            // Invariant 5 (specs/GH686/product.md): the match succeeded but the
            // gallery entry cannot be turned into a valid template. Surface the
            // failure to the user rather than silently returning.
            self.add_error_toast(
                crate::t!(
                    "settings-mcp-install-from-link-failed",
                    name = gallery_title
                ),
                ctx,
            );
            return;
        };
        log::info!(
            "Deeplink autoinstall for '{gallery_title}': installation modal removed, request will be rejected"
        );
        self.start_server_installation(
            templatable_mcp_server,
            instructions,
            InstallOrigin::Deeplink,
            ctx,
        );
    }

    fn handle_list_view_event(
        &mut self,
        event: &MCPServersListPageViewEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            MCPServersListPageViewEvent::Edit(mcp_item_id) => {
                self.update_page(
                    MCPServersSettingsPage::Edit {
                        item_id: Some(*mcp_item_id),
                    },
                    ctx,
                );
            }
            MCPServersListPageViewEvent::Add => {
                self.update_page(MCPServersSettingsPage::Edit { item_id: None }, ctx);
            }
            MCPServersListPageViewEvent::LogOut(server_card_item_id, server_name) => {
                self.handle_log_out(*server_card_item_id, Some(server_name.clone()), ctx);
            }
            MCPServersListPageViewEvent::StartInstallation {
                templatable_mcp_server: template,
                instructions_in_markdown,
                origin,
            } => {
                self.start_server_installation(
                    template.clone(),
                    instructions_in_markdown.clone(),
                    *origin,
                    ctx,
                );
            }
        }
    }

    fn handle_edit_view_event(
        &mut self,
        event: &MCPServersEditPageViewEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            MCPServersEditPageViewEvent::Back => {
                self.update_page(MCPServersSettingsPage::List, ctx);
            }
            MCPServersEditPageViewEvent::Reinstall(template_uuid) => {
                self.reinstall_server(*template_uuid, ctx);
            }
            MCPServersEditPageViewEvent::Delete(item_id) => {
                self.list_view.update(ctx, |list_view, ctx| {
                    list_view.delete_server(*item_id, ctx);
                });
                self.update_page(MCPServersSettingsPage::List, ctx);
            }
            MCPServersEditPageViewEvent::LogOut(server_card_item_id, server_name) => {
                self.handle_log_out(*server_card_item_id, server_name.clone(), ctx);
            }
        }
    }

    /// W3 域裁切:两个 MCP 模态(installation / update)均已物理删,
    /// 设置页不再贡献任何模态内容。
    pub fn get_modal_content(&self, _app: &AppContext) -> Option<Box<dyn Element>> {
        None
    }
}

impl Entity for MCPServersSettingsPageView {
    type Event = ();
}

impl View for MCPServersSettingsPageView {
    fn ui_name() -> &'static str {
        "MCPServersSettingsPageView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        match self.current_page {
            MCPServersSettingsPage::List => self.page.render(self, _app),
            MCPServersSettingsPage::Edit { item_id: _ } => {
                // The edit view needs to be constrained so we will render it directly
                // instead of rendering inside the settings widget
                Container::new(ChildView::new(&self.edit_view).finish())
                    .with_uniform_padding(style::PAGE_PADDING)
                    .finish()
            }
        }
    }
}

impl TypedActionView for MCPServersSettingsPageView {
    type Action = ();
}

impl SettingsPageMeta for MCPServersSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::MCPServers
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget()
    }
}


#[derive(Default)]
pub struct MCPServersSettingsWidget {
    // No state yet
}

impl SettingsWidget for MCPServersSettingsWidget {
    type View = MCPServersSettingsPageView;

    fn search_terms(&self) -> &str {
        "mcp servers"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        // The settings widget will always return list view
        // The edit view needs to be constrained so we will render it directly
        // instead of rendering inside the settings widget
        ChildView::new(&view.list_view).finish()
    }
}
