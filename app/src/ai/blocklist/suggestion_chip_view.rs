use crate::ai::agent::{SuggestedLoggingId, SuggestedRule};
use crate::ai::facts::AIFactObjectModel;
use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::model::persistence::{ObjectStoreEvent, ObjectStoreModel};
use crate::cloud_object::update_manager::{
    ObjectOperation, OperationSuccessType, UpdateManagerEvent,
};
use crate::drive::ObjectTypeAndId;
use crate::server::ids::SyncId;
use crate::view_components::action_button::{ActionButton, ActionButtonTheme, SecondaryTheme};
use crate::{
    ai::facts::{AIFact, AIMemory},
    cloud_object::update_manager::UpdateManager,
    server::ids::ClientId,
    ui_components::{blended_colors, icons::Icon},
};
use pathfinder_color::ColorU;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill;
use warpui::{
    elements::{Align, ChildView, Container, ParentElement, SavePosition, Stack},
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use super::suggested_rule_modal::SuggestedRuleAndId;

const MAX_CHIP_WIDTH: f32 = 316.;

/// A chip view component for displaying suggested rules.
///
/// This component is responsible for:
/// - Rendering a clickable chip that represents a suggested rule
/// - Tracking the state of the rule (saved or not)
/// - Handling clicks to show the appropriate modal dialog via workspace-level events
/// - Syncing with the local object store for persistence
///
/// When clicked, the chip emits events to display the `SuggestedRuleModal` at the
/// workspace level, which positions itself relative to the chip position.
///
/// # UI Behavior
/// - A chip with a saved rule shows a check icon and has a different theme
/// - The chip position is saved for the modal dialogs to position relative to it
/// - Chips have limited width and tooltips for overflow content
///
/// # Events
/// The component emits events to:
/// - Show rule modals
/// - Open existing rules directly
///
/// An [`ActionButton`] theme for suggested rules.
struct SuggestionButtonTheme;

impl ActionButtonTheme for SuggestionButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        if hovered {
            Some(blended_colors::fg_overlay_2(appearance.theme()))
        } else {
            None
        }
    }

    fn text_color(
        &self,
        _hovered: bool,
        _background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        appearance
            .theme()
            .sub_text_color(appearance.theme().background())
            .into()
    }

    fn border(&self, appearance: &Appearance) -> Option<ColorU> {
        Some(blended_colors::neutral_4(appearance.theme()))
    }
}

/// A theme for the dismiss button in the suggestion footer.
pub struct SuggestionDismissButtonTheme;

impl ActionButtonTheme for SuggestionDismissButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        if hovered {
            Some(blended_colors::fg_overlay_2(appearance.theme()))
        } else {
            None
        }
    }

    fn text_color(
        &self,
        _hovered: bool,
        _background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        appearance
            .theme()
            .sub_text_color(appearance.theme().background())
            .into()
    }
}

#[derive(Debug, Clone)]
pub enum SuggestedChipViewEvent {
    ShowSuggestedRuleDialog {
        rule_and_id: SuggestedRuleAndId,
    },
    OpenAIFactCollection {
        sync_id: Option<SyncId>,
    },
}

#[derive(Debug, Clone)]
pub enum SuggestedViewAction {
    ChipClicked,
}

#[derive(Debug, Clone)]
enum Suggestion {
    Rule {
        rule: SuggestedRule,
    },
}

impl Suggestion {
    pub fn icon(&self) -> Icon {
        match self {
            Suggestion::Rule { .. } => Icon::BookOpen,
        }
    }

    pub fn tooltip(&self) -> String {
        match self {
            Suggestion::Rule { rule, .. } => {
                format!("Add rule: {}", rule.content.clone())
            }
        }
    }

    fn position_id(&self) -> String {
        match self {
            Suggestion::Rule { rule, .. } => format!("rule_position_{}", rule.logging_id),
        }
    }

    fn chip_label(&self) -> String {
        match self {
            Suggestion::Rule { rule, .. } => rule.content.clone(),
        }
    }
}

/// Data required to render a single suggested rule.
#[derive(Clone)]
pub struct SuggestionChipView {
    suggestion: Suggestion,
    chip: ViewHandle<ActionButton>,
    sync_id: SyncId,
    is_saved: bool,
}

impl SuggestionChipView {
    pub fn new_rule_chip(rule: SuggestedRule, ctx: &mut ViewContext<Self>) -> Self {
        Self::listen_for_warp_drive_events(ctx);

        let chip = ctx.add_typed_action_view(|_| {
            ActionButton::new(rule.content.clone(), SecondaryTheme)
                .with_max_label_width(MAX_CHIP_WIDTH)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(SuggestedViewAction::ChipClicked);
                })
        });

        let suggestion = Suggestion::Rule { rule };
        let mut me = Self {
            suggestion,
            sync_id: SyncId::ClientId(ClientId::default()),
            chip,
            is_saved: false,
        };
        me.reset_suggestion(ctx);
        me
    }

    pub fn logging_id(&self) -> SuggestedLoggingId {
        match &self.suggestion {
            Suggestion::Rule { rule, .. } => rule.logging_id.clone(),
        }
    }

    fn listen_for_warp_drive_events(ctx: &mut ViewContext<Self>) {
        let update_manager = UpdateManager::handle(ctx);
        ctx.subscribe_to_model(&update_manager, |me, _, event, ctx| {
            me.handle_update_manager_event(event, ctx);
        });

        let object_model = ObjectStoreModel::handle(ctx);
        ctx.subscribe_to_model(&object_model, |me, _, event, ctx| {
            me.handle_object_store_event(event, ctx);
        });
    }

    fn handle_update_manager_event(
        &mut self,
        event: &UpdateManagerEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        let UpdateManagerEvent::ObjectOperationComplete { result } = event else {
            return;
        };

        if let (ObjectOperation::Create { .. }, OperationSuccessType::Success) =
            (&result.operation, &result.success_type)
        {
            if self.sync_id.into_client() == result.client_id {
                if let Some(server_id) = result.server_id {
                    self.sync_id = SyncId::ServerId(server_id);
                    // Reload the rule from the local object store.
                    match &mut self.suggestion {
                        Suggestion::Rule { .. } => {
                            self.load_suggestion(ctx);
                        }
                    }
                    self.on_add_suggestion(ctx);
                }
            }
        }
    }

    fn handle_object_store_event(&mut self, event: &ObjectStoreEvent, ctx: &mut ViewContext<Self>) {
        match event {
            ObjectStoreEvent::ObjectUpdated {
                type_and_id: ObjectTypeAndId::GenericStringObject { id, .. },
                ..
            } => {
                if self.sync_id.into_client() == id.into_client() {
                    self.load_suggestion(ctx);
                }
            }
            ObjectStoreEvent::ObjectTrashed {
                type_and_id: ObjectTypeAndId::GenericStringObject { id, .. },
                ..
            }
            | ObjectStoreEvent::ObjectDeleted {
                type_and_id: ObjectTypeAndId::GenericStringObject { id, .. },
                ..
            } => {
                // If the rule has been deleted, then we should reset it such that
                // the suggestion can be added again.
                if self.sync_id == *id {
                    self.reset_suggestion(ctx);
                }
            }
            _ => {}
        }
    }

    /// Resets the rule to its initial state.
    fn reset_suggestion(&mut self, ctx: &mut ViewContext<Self>) {
        self.sync_id = SyncId::ClientId(ClientId::default());
        self.is_saved = false;
        let icon: Icon = self.suggestion.icon();
        let tooltip: String = self.suggestion.tooltip();
        let label: String = self.suggestion.chip_label();
        self.chip.update(ctx, |chip, ctx| {
            chip.set_icon(Some(icon), ctx);
            chip.set_label(label.clone(), ctx);
            chip.set_tooltip(Some(tooltip.clone()), ctx);
            chip.set_theme(SecondaryTheme, ctx);
            chip.set_active(false, ctx);
        });
        ctx.notify();
    }

    /// Fetches the rule from the local object store, and updates the UI to reflect that.
    fn load_suggestion(&mut self, ctx: &mut ViewContext<Self>) {
        let object_model = ObjectStoreModel::handle(ctx);
        let tooltip = self.suggestion.tooltip();

        match &mut self.suggestion {
            Suggestion::Rule { .. } => {
                if let Some(rule) = object_model
                    .as_ref(ctx)
                    .get_object_of_type::<GenericStringObjectId, AIFactObjectModel>(&self.sync_id)
                {
                    let AIFact::Memory(AIMemory { content, .. }) =
                        rule.model().string_model.clone();
                    self.chip.update(ctx, |chip, ctx| {
                        chip.set_label(content.clone(), ctx);
                        chip.set_tooltip(Some(tooltip.clone()), ctx);
                    });
                    ctx.notify();
                }
            }
        }
    }

    /// Updates the UI state to reflect that a rule has been added.
    fn on_add_suggestion(&mut self, ctx: &mut ViewContext<Self>) {
        self.is_saved = true;
        self.chip.update(ctx, |chip, ctx| {
            chip.set_icon(Some(Icon::Check), ctx);
            chip.set_theme(SuggestionButtonTheme, ctx);
        });
        ctx.notify();
    }
}

impl Entity for SuggestionChipView {
    type Event = SuggestedChipViewEvent;
}

impl View for SuggestionChipView {
    fn ui_name() -> &'static str {
        "SuggestionChipView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        let stack = Stack::new();
        let position_id = self.suggestion.position_id();

        let chip_element = Container::new(ChildView::new(&self.chip).finish())
            .with_margin_right(8.)
            .finish();

        stack
            .with_child(
                Align::new(
                    Container::new(SavePosition::new(chip_element, position_id.as_str()).finish())
                        .finish(),
                )
                .left()
                .finish(),
            )
            .finish()
    }
}

impl TypedActionView for SuggestionChipView {
    type Action = SuggestedViewAction;

    fn handle_action(&mut self, action: &SuggestedViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            SuggestedViewAction::ChipClicked => match &self.suggestion {
                Suggestion::Rule { rule, .. } => {
                    if ObjectStoreModel::as_ref(ctx)
                        .get_object_of_type::<GenericStringObjectId, AIFactObjectModel>(
                            &self.sync_id,
                        )
                        .is_some()
                    {
                        ctx.emit(SuggestedChipViewEvent::OpenAIFactCollection {
                            sync_id: Some(self.sync_id),
                        });
                    } else {
                        ctx.emit(SuggestedChipViewEvent::ShowSuggestedRuleDialog {
                            rule_and_id: SuggestedRuleAndId {
                                rule: rule.clone(),
                                sync_id: self.sync_id,
                            },
                        });
                        self.chip.update(ctx, |chip, ctx| {
                            chip.set_active(true, ctx);
                        });
                    }
                }
            },
        }
    }
}
