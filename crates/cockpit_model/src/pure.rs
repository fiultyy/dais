//! 纯函数派生 (自 app/src/ai/cockpit/model.rs 原样迁移, 除 recap_chain 去
//! CLIAgentSessionContext 参数化为显式字段)。全部 (输入→输出) 确定性:
//! 同输入必同输出, 无隐藏状态 — 测试即幂等证明。

use crate::{CockpitCard, CockpitCardGroup, CockpitGroupBy, CockpitSort, CockpitStatusFilter};

/// preview 尾行:active block 输出的最后一条非空行(单行,已 trim)。
pub fn preview_tail_from_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
}

/// recap 四级回退(spec §4.2):`response > query > summary > preview_tail`。
/// (app 侧传入 session context 的三个字段 + preview_tail。)
pub fn recap_chain(
    response: Option<&String>,
    query: Option<&String>,
    summary: Option<&String>,
    preview_tail: Option<String>,
) -> Option<String> {
    response
        .cloned()
        .or_else(|| query.cloned())
        .or_else(|| summary.cloned())
        .or(preview_tail)
}

/// 文本筛选:标题/cwd/agent 名/recap/tool 不区分大小写子串匹配。
pub fn card_matches_filter(card: &CockpitCard, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_lowercase();
    let matches =
        |haystack: Option<&str>| haystack.is_some_and(|h| h.to_lowercase().contains(&needle));
    card.title.to_lowercase().contains(&needle)
        || matches(card.cwd.as_deref())
        || matches(card.agent_name)
        || matches(card.recap.as_deref())
        || matches(card.tool_name.as_deref())
}

/// 分组键:cwd 末段目录名(项目/worktree 粒度;未上报 = "?")。
pub fn cwd_group_key(cwd: &Option<String>) -> String {
    cwd.as_deref()
        .and_then(|c| c.rsplit('/').find(|segment| !segment.is_empty()))
        .unwrap_or("?")
        .to_string()
}

/// 就地应用视图参数:筛选 → 排序(分组开启时分组键为主键,保证组连续)。
pub fn apply_view_params(
    cards: &mut Vec<CockpitCard>,
    filter: &str,
    status_filter: Option<CockpitStatusFilter>,
    sort: CockpitSort,
    group_by: CockpitGroupBy,
) {
    cards.retain(|card| {
        card_matches_filter(card, filter)
            && status_filter.is_none_or(|kind| card.status.kind() == kind)
    });
    match group_by {
        CockpitGroupBy::None => sort_cards(cards, sort),
        CockpitGroupBy::CwdProject => {
            let mut keyed: Vec<(String, CockpitCard)> =
                cards.drain(..).map(|c| (cwd_group_key(&c.cwd), c)).collect();
            keyed.sort_by(|(ka, a), (kb, b)| ka.cmp(kb).then_with(|| sort_cards_cmp(a, b, sort)));
            cards.extend(keyed.into_iter().map(|(_, c)| c));
        }
    }
}

/// 排序比较(不含分组键;末级用 EntityId 断平,保证稳定序)。
pub fn sort_cards_cmp(a: &CockpitCard, b: &CockpitCard, sort: CockpitSort) -> std::cmp::Ordering {
    match sort {
        CockpitSort::Activity => a
            .status
            .sort_rank()
            .cmp(&b.status.sort_rank())
            .then_with(|| a.title.cmp(&b.title)),
        CockpitSort::Title => a.title.cmp(&b.title),
        CockpitSort::Cwd => a.cwd.cmp(&b.cwd).then_with(|| a.title.cmp(&b.title)),
    }
    .then_with(|| a.terminal_view_id.cmp(&b.terminal_view_id))
}

pub fn sort_cards(cards: &mut [CockpitCard], sort: CockpitSort) {
    cards.sort_by(|a, b| sort_cards_cmp(a, b, sort));
}

/// 由连续分组键切出分组区间(分组关闭 → 单个全量组)。
pub fn compute_groups(cards: &[CockpitCard], group_by: CockpitGroupBy) -> Vec<CockpitCardGroup> {
    match group_by {
        CockpitGroupBy::None => vec![CockpitCardGroup {
            key: String::new(),
            range: 0..cards.len(),
        }],
        CockpitGroupBy::CwdProject => {
            let mut groups = Vec::new();
            let mut start = 0usize;
            let mut current_key: Option<String> = None;
            for (idx, card) in cards.iter().enumerate() {
                let key = cwd_group_key(&card.cwd);
                match &current_key {
                    Some(k) if *k == key => {}
                    Some(_) => {
                        groups.push(CockpitCardGroup {
                            key: current_key.clone().unwrap_or_default(),
                            range: start..idx,
                        });
                        start = idx;
                        current_key = Some(key);
                    }
                    None => current_key = Some(key),
                }
            }
            if let Some(key) = current_key {
                groups.push(CockpitCardGroup {
                    key,
                    range: start..cards.len(),
                });
            }
            groups
        }
    }
}

/// 注入目标解析:选中集 ∩ 全量快照,按卡片顺序稳定输出(选中集本身无序)。
pub fn resolve_targets(
    all_cards: &[CockpitCard],
    selected_set: &std::collections::HashSet<warpui::EntityId>,
) -> Vec<warpui::EntityId> {
    all_cards
        .iter()
        .filter(|card| selected_set.contains(&card.terminal_view_id))
        .map(|card| card.terminal_view_id)
        .collect()
}
