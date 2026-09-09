use itertools::Itertools as _;
use warpui::{AppContext, SingletonEntity as _};

use super::current_prompt::CurrentPrompt;
use super::prompt::Prompt;
use terminal_grid_model::prompt_snapshot::ChipResult;

pub use terminal_grid_model::prompt_snapshot::PromptSnapshot;

/// R21: from_current_prompt 依赖 app Prompt/CurrentPrompt,改自由函数(孤儿规则)。
pub fn prompt_snapshot_from_current_prompt(
    current_prompt: &CurrentPrompt,
    ctx: &AppContext,
) -> PromptSnapshot {

        let prompt = Prompt::as_ref(ctx);
        let current_prompt_snapshot = current_prompt.snapshot();
        let current_prompt_on_click_snapshot = current_prompt.on_click_snapshot();

        // Get base chip kinds from prompt configuration
        let all_chip_kinds = prompt.chip_kinds();

        // Re-sort current prompt snapshot so that it matches the order of elements in prompt
        let chips = all_chip_kinds
            .iter()
            .map(|chip_kind| {
                let value = current_prompt_snapshot
                    .get(chip_kind)
                    .cloned()
                    .unwrap_or_default();
                let on_click_values = current_prompt_on_click_snapshot
                    .get(chip_kind)
                    .cloned()
                    .unwrap_or_default();
                ChipResult {
                    kind: chip_kind.clone(),
                    value,
                    on_click_values,
                }
            })
            .collect_vec();

        log::debug!("Current prompt snapshot: {chips:?}");
        PromptSnapshot {
            chips,
            same_line_prompt_enabled: current_prompt.same_line_prompt_enabled(),
            separator: current_prompt.separator(),
        }
    }
