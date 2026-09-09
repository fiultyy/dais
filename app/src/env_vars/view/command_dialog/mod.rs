use warpui::ViewContext;

use super::env_var_collection::{EnvVarCollectionView, VariableRowIndex};
use crate::env_vars::{active_env_var_collection_data::SavingStatus, EnvVarValue};

mod command_dialog_view;
pub(super) use command_dialog_view::{EnvVarCommandDialog, EnvVarCommandDialogEvent};

// P-ARC R1: 定义上移 env_vars_ui::model(数据模型 serde 硬依赖);
// re-export 保持 `crate::env_vars::view::command_dialog::EnvVarSecretCommand`
// 既有路径(requested_command_attribution_test 消费)。
pub use env_vars_ui::EnvVarSecretCommand;

impl EnvVarCollectionView {
    pub(super) fn display_command_dialog(
        &mut self,
        index: Option<VariableRowIndex>,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(VariableRowIndex(index)) = index {
            if let EnvVarValue::Command(cmd) = &self.variable_rows[index].value {
                self.env_var_command_dialog
                    .update(ctx, |dialog, ctx| dialog.load(cmd, ctx))
            }
        }
        self.dialog_open_states.env_var_command_dialog_open = true;
        self.update_open_modal_state(ctx);
        ctx.focus(&self.env_var_command_dialog);
        ctx.notify();
    }

    pub(super) fn handle_command_dialog_event(
        &mut self,
        event: &EnvVarCommandDialogEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EnvVarCommandDialogEvent::Close => {
                self.dialog_open_states.env_var_command_dialog_open = false;
                self.update_open_modal_state(ctx);
                ctx.notify();
            }
            EnvVarCommandDialogEvent::SaveCommand(command) => {
                self.save_command(command.clone(), ctx);
                self.set_saving_status(SavingStatus::Unsaved, ctx)
            }
        }
    }

    fn save_command(&mut self, command: EnvVarSecretCommand, ctx: &mut ViewContext<Self>) {
        let row_index = self.pending_variable_row_index.take();

        if let Some(VariableRowIndex(index)) = row_index {
            self.variable_rows[index].value = EnvVarValue::Command(command);
        }
        ctx.notify();
    }
}
