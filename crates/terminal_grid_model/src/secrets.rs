#![allow(dead_code)]

use terminal_model::index::Point;
use anyhow::anyhow;
use itertools::Itertools;
use lazy_static::lazy_static;
use parking_lot::RwLock;
use rangemap::{RangeInclusiveMap, StepLite};
use std::collections::HashMap;
use std::ops::{Not, RangeInclusive};
use std::sync::atomic::{AtomicUsize, Ordering};
use warpui::elements::SecretRange;
use warpui::EntityId;

use crate::grid::grid_handler::GridHandler;
use crate::grid::{Dimensions as _, RespectDisplayedOutput};
use terminal_model::RangeInModel;
use crate::find::RegexDFAs;

/// Tracks counts to infer which regex patterns correspond to which secret levels
#[derive(Debug, Clone)]
pub struct RegexLevelMetadata {
    /// Number of enterprise regex patterns (they are added first)
    pub enterprise_count: usize,
    /// Number of user regex patterns (they are added after enterprise patterns)
    pub user_count: usize,
}

lazy_static! {
    /// Used for secret redaction in the Grid.
    /// Initially empty - will be populated with user-defined regexes when safe mode is enabled.
    pub static ref SECRETS_DFA: RwLock<RegexDFAs> = RwLock::new(
        RegexDFAs::new_many(&[], true, true)
            .expect("should be able to construct empty regex DFA")
    );
    /// Used for secret redaction in simple text strings (e.g.: rich content blocks).
    /// Initially empty - will be populated with user-defined regexes when safe mode is enabled.
    pub static ref SECRETS_REGEX: RwLock<regex_automata::meta::Regex> = RwLock::new(
        regex_automata::meta::Regex::new_many(&[] as &[&str])
            .expect("should be able to construct empty regex")
    );
    /// Tracks counts to infer which regex patterns correspond to which secret levels
    pub static ref REGEX_LEVEL_METADATA: RwLock<RegexLevelMetadata> = RwLock::new(
        RegexLevelMetadata {
            enterprise_count: 0,
            user_count: 0,
        }
    );
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd)]
/// A handle to a [`Secret`].
pub struct SecretHandle(usize);

impl SecretHandle {
    pub fn next() -> Self {
        static SECRET_HANDLE: AtomicUsize = AtomicUsize::new(0);
        let next = SECRET_HANDLE.fetch_add(1, Ordering::Relaxed);
        SecretHandle(next)
    }

    pub fn id(&self) -> String {
        format!("{}", self.0)
    }
}


#[derive(Copy, Clone, Debug)]
pub enum IsObfuscated {
    Yes,
    No,
}

/// Represents the level/source of a secret redaction rule
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SecretLevel {
    /// User-defined custom secret patterns
    User,
    /// Enterprise/organization-defined secret patterns
    Enterprise,
}

impl SecretLevel {
    /// Returns true if this is an enterprise level secret
    pub fn is_enterprise(self) -> bool {
        matches!(self, SecretLevel::Enterprise)
    }

    /// Returns true if this is a user level secret
    pub fn is_user(self) -> bool {
        matches!(self, SecretLevel::User)
    }

    /// Returns the priority of the secret level. Enterprise has highest priority.
    pub fn priority(self) -> u8 {
        match self {
            SecretLevel::User => 0,
            SecretLevel::Enterprise => 1,
        }
    }
}

/// Whether or not to respect obfuscated secrets when retrieving grid contents.
#[derive(Copy, Clone, PartialEq)]
pub enum RespectObfuscatedSecrets {
    No,
    Yes,
}

/// Whether or not to obfuscate secrets during grid and tooltip rendering, respecting the Safe Mode setting.
#[derive(Clone, Copy, Debug, Default)]
pub enum ObfuscateSecrets {
    // Identify and visually obfuscate secrets
    Yes,
    /// Do not visually obfuscate secrets, but highlight them with a strikethrough
    Strikethrough,
    /// Show secrets with normal styling but still detect them for interaction (no visual treatment)
    AlwaysShow,
    #[default]
    No,
}

impl Not for ObfuscateSecrets {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            ObfuscateSecrets::Yes => ObfuscateSecrets::No,
            ObfuscateSecrets::No => ObfuscateSecrets::Yes,
            ObfuscateSecrets::Strikethrough => ObfuscateSecrets::Yes,
            ObfuscateSecrets::AlwaysShow => ObfuscateSecrets::Yes,
        }
    }
}

impl ObfuscateSecrets {
    /// Returns the "stronger" obfuscation mode. Priority: Yes > Strikethrough > AlwaysShow > No
    pub fn and(&self, other: &ObfuscateSecrets) -> ObfuscateSecrets {
        match (self, other) {
            (ObfuscateSecrets::Yes, _) | (_, ObfuscateSecrets::Yes) => ObfuscateSecrets::Yes,
            (ObfuscateSecrets::Strikethrough, _) | (_, ObfuscateSecrets::Strikethrough) => {
                ObfuscateSecrets::Strikethrough
            }
            (ObfuscateSecrets::AlwaysShow, _) | (_, ObfuscateSecrets::AlwaysShow) => {
                ObfuscateSecrets::AlwaysShow
            }
            (ObfuscateSecrets::No, ObfuscateSecrets::No) => ObfuscateSecrets::No,
        }
    }

    /// Returns whether the secret should be redacted given the current safe mode settings.
    /// This includes obfuscation, strikethrough, and always show (for interaction purposes).
    pub fn should_redact_secret(&self) -> bool {
        matches!(
            self,
            ObfuscateSecrets::Yes | ObfuscateSecrets::Strikethrough | ObfuscateSecrets::AlwaysShow
        )
    }

    /// Returns whether the current obfuscation mode is `ObfuscateSecrets::Yes`
    pub fn is_visually_obfuscated(&self) -> bool {
        matches!(self, ObfuscateSecrets::Yes)
    }
}

/// A secret (API key, password, etc) contained within the grid.
#[derive(Clone, Debug)]
pub struct Secret {
    /// Whether the secret is currently obfuscated.
    is_obfuscated: IsObfuscated,
    range: RangeInclusive<Point>,
    /// The level/source of this secret's redaction rule
    secret_level: SecretLevel,
}

impl RangeInModel for &Secret {
    fn range(&self) -> RangeInclusive<Point> {
        self.range.clone()
    }
}

impl RangeInModel for &mut Secret {
    fn range(&self) -> RangeInclusive<Point> {
        self.range.clone()
    }
}

pub type SecretAndHandle<'a> = (SecretHandle, &'a Secret);

impl Secret {
    pub fn set_is_obfuscated(&mut self, is_obfuscated: IsObfuscated) {
        self.is_obfuscated = is_obfuscated
    }

    pub fn is_obfuscated(&self) -> bool {
        matches!(self.is_obfuscated, IsObfuscated::Yes)
    }

    pub fn new(
        is_obfuscated: IsObfuscated,
        range: RangeInclusive<Point>,
        secret_level: SecretLevel,
    ) -> Self {
        Self {
            is_obfuscated,
            range,
            secret_level,
        }
    }

    pub fn secret_level(&self) -> SecretLevel {
        self.secret_level
    }
}

/// Map that is responsible for storing secrets indexed by both [`SecretHandle`] and `Range`.
#[derive(Clone, Default, Debug)]
pub struct SecretMap {
    /// Mapping of secrets stored within the grid, keyed on the secret's [`SecretHandle`].
    secrets: HashMap<SecretHandle, Secret>,
    /// Mapping of secrets keyed on the range of the secret.
    secret_ranges: RangeInclusiveMap<RangeMapPoint, SecretHandle>,
}

impl SecretMap {
    /// Insert a [`Secret`] identified by `handle` into the map.
    pub fn insert(&mut self, handle: SecretHandle, secret: Secret, num_columns: usize) {
        let secret_range = secret.range.clone();
        let range_point_range = RangeMapPoint::new(*secret_range.start(), num_columns)
            ..=RangeMapPoint::new(*secret_range.end(), num_columns);
        self.secret_ranges.insert(range_point_range, handle);
        self.secrets.insert(handle, secret);
    }

    /// Removes a [`Secret`] identified by `handle` from the map.
    pub fn remove(&mut self, handle: SecretHandle, num_columns: usize) {
        let removed = self.secrets.remove(&handle);
        if let Some(secret) = removed {
            let range = RangeMapPoint::new(*secret.range.start(), num_columns)
                ..=RangeMapPoint::new(*secret.range.end(), num_columns);
            self.secret_ranges.remove(range);
        }
    }

    /// Returns the [`Secret`] identified by [`SecretHandle`] or `None` if no such secret exists.
    pub fn get_by_handle(&self, handle: &SecretHandle) -> Option<&Secret> {
        self.secrets.get(handle)
    }

    /// Returns the [`Secret`] and its corresponding [`SecretHandle`] contained at the current
    /// [`Point`]. Returns `None` if there is no secret at the given point.
    pub fn get_by_point(
        &self,
        point: Point,
        grid: &GridHandler,
        respect_displayed_output: RespectDisplayedOutput,
    ) -> Option<SecretAndHandle<'_>> {
        let original_point = if grid.has_displayed_output()
            && matches!(respect_displayed_output, RespectDisplayedOutput::Yes)
        {
            grid.maybe_translate_point_from_displayed_to_original(point)
        } else {
            point
        };
        let point_with_metadata = RangeMapPoint::new(original_point, grid.columns());
        let handle = self.secret_ranges.get(&point_with_metadata).copied();

        handle.zip(handle.and_then(|h| self.get_by_handle(&h)))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SecretHandle, &Secret)> {
        self.secrets.iter()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn ranges(&self) -> impl Iterator<Item = (RangeInclusive<Point>, &SecretHandle)> {
        self.secret_ranges
            .iter()
            .map(|(range, handle)| (range.start().as_point()..=range.end().as_point(), handle))
    }

    /// Clears all secrets within the map.
    pub fn clear(&mut self) {
        self.secrets.clear();
        self.secret_ranges.clear();
    }

    /// Marks the secret identified by `handle` as obfuscated. Returns an `Err` if no secret is
    /// identified by the `handle`.
    pub fn set_is_obfuscated(
        &mut self,
        handle: &SecretHandle,
        is_obfuscated: IsObfuscated,
    ) -> anyhow::Result<()> {
        let secret = self
            .secrets
            .get_mut(handle)
            .ok_or_else(|| anyhow!("No secret identified by provided SecretHandle"))?;
        secret.is_obfuscated = is_obfuscated;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Clears all of the secret ranges. Should be called after the resize of a grid since ranges
    /// are not stable across resizes.
    pub fn clear_ranges_after_resize(&mut self) {
        self.secret_ranges.clear();
    }
}

/// Updates secret scanning to separate user-defined regexes from enterprise ones.
/// Ensures enterprise secrets are handled differently, maintaining separation from user settings.
/// If the internal [`RegexDFAs`] can't be constructed from the new regexes for any reason,
/// the current DFA is kept unchanged.
pub fn set_user_and_enterprise_secret_regexes<'a>(
    user_secrets: impl IntoIterator<Item = &'a regex::Regex>,
    enterprise_secrets: impl IntoIterator<Item = &'a regex::Regex>,
) {
    let mut secrets = SECRETS_DFA.write();

    // Collect enterprise and user secrets into vectors to count them
    let enterprise_secrets_vec: Vec<&'a regex::Regex> = enterprise_secrets.into_iter().collect();
    let user_secrets_vec: Vec<&'a regex::Regex> = user_secrets.into_iter().collect();

    // Dedup user regex entries against enterprise regexes to improve performance
    let mut seen_patterns: std::collections::HashSet<&str> =
        enterprise_secrets_vec.iter().map(|r| r.as_str()).collect();

    let filtered_user_secrets_vec: Vec<&'a regex::Regex> = user_secrets_vec
        .into_iter()
        .filter(|r| seen_patterns.insert(r.as_str()))
        .collect();

    // Combine all secrets additively: enterprise first (highest priority), then filtered user
    let all_secrets = enterprise_secrets_vec
        .iter()
        .map(|regex| regex.as_str())
        .chain(filtered_user_secrets_vec.iter().map(|regex| regex.as_str()))
        .collect_vec();

    // Update the metadata counts first to ensure it's ready when the new regexes are set
    let mut metadata = REGEX_LEVEL_METADATA.write();
    metadata.enterprise_count = enterprise_secrets_vec.len();
    metadata.user_count = filtered_user_secrets_vec.len();

    let mut secrets_regex = SECRETS_REGEX.write();
    match regex_automata::meta::Regex::new_many(&all_secrets) {
        Ok(regex) => {
            *secrets_regex = regex;
        }
        Err(err) => {
            log::error!("Failed to construct new Regex with combined secrets: {err:?}");
        }
    }

    match RegexDFAs::new_many(&all_secrets, true, true) {
        Ok(dfa) => {
            *secrets = dfa;
        }
        Err(err) => {
            log::error!("Failed to construct new RegexDFA with combined secrets: {err:?}");
        }
    };
}

/// A wrapper around a [`Point`] that implements [`StepLite`], allowing us to store it in a
/// `RangeMap`. Used for secret redaction so we efficiently map from a given range to an underlying
/// secret stored at that range.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
struct RangeMapPoint {
    point: Point,
    num_cols: usize,
}

impl RangeMapPoint {
    fn new(point: Point, num_cols: usize) -> Self {
        Self { point, num_cols }
    }

    fn as_point(&self) -> Point {
        self.point
    }
}

impl StepLite for RangeMapPoint {
    fn add_one(&self) -> Self {
        let mut new_point = self.point;
        new_point.col += 1;
        if new_point.col >= self.num_cols {
            new_point.col = 0;
            new_point.row += 1;
        }

        RangeMapPoint {
            point: new_point,
            num_cols: self.num_cols,
        }
    }

    fn sub_one(&self) -> Self {
        let mut new_point = self.point;
        if new_point.col == 0 {
            if new_point.row == 0 {
                return *self;
            }
            new_point.row -= 1;
            new_point.col = self.num_cols - 1;
        } else {
            new_point.col -= 1;
        }

        RangeMapPoint {
            point: new_point,
            num_cols: self.num_cols,
        }
    }
}


/// R10 缝默认实现: DFA 匹配不携带 level 元数据(app 版 regex 元数据在
/// ai::blocklist 侧),与原 `unwrap_or(SecretLevel::User)` 兜底语义一致。
/// crate 内测试与 app 未注入时使用。
pub fn dfa_determine_secret_level(_plaintext: &str) -> SecretLevel {
    SecretLevel::User
}

/// R14: 自 app model/secrets_tooltip.rs 下沉(原 include! 内联)。
#[derive(Clone, Debug)]
pub struct RichContentSecretTooltipInfo {
    pub secret: String,
    pub secret_range: warpui::elements::SecretRange,
    pub location: crate::TextLocation,
    pub is_obfuscated: bool,
    pub position_id: String,
    pub view_id: warpui::EntityId,
    pub secret_level: SecretLevel,
}

// ---- R15: 文本 secret 检测链(自 app/ai/blocklist/block/secret_redaction.rs 提升) ----

pub const SECRET_REDACTION_REPLACEMENT_CHARACTER: &str = "*";

/// Returns the ranges of detected secrets in the given text.
pub fn find_secrets_in_text(text: &str) -> Vec<SecretRange> {
    find_secrets_in_text_with_levels(text)
        .into_iter()
        .map(|(range, _level)| range)
        .collect()
}

/// Returns the ranges of detected secrets in the given text along with their SecretLevel.
pub fn find_secrets_in_text_with_levels(text: &str) -> Vec<(SecretRange, SecretLevel)> {
    // Combine all regex patterns into a single regex pattern with non-capturing groups, for efficiency.
    // Note that we purposely use regex::Regex instead of RegexDFAs since we are working a Text (containing
    // a normal String) rather than the Grid (where text is in Cells with 1 character each).
    let regex = SECRETS_REGEX.read();
    let metadata = REGEX_LEVEL_METADATA.read();

    let mut secret_ranges = vec![];
    let mut byte_to_char_index = vec![0; text.len() + 1]; // Map byte index to char index

    // Track the current character index while iterating through the string.
    let mut char_index = 0;
    for (byte_index, _) in text.char_indices() {
        byte_to_char_index[byte_index] = char_index;
        char_index += 1;
    }
    byte_to_char_index[text.len()] = char_index; // Map the last byte to the last character index

    // Iterate over the text once, finding all matches against secret regex. Map the byte ranges
    // to character ranges and store them.
    for mat in regex.find_iter(text) {
        let start_byte = mat.start();
        let end_byte = mat.end();
        let start_char = byte_to_char_index[start_byte];
        let end_char = byte_to_char_index[end_byte];

        // Determine which pattern matched by getting the pattern ID and map via counts
        let pattern_id = mat.pattern().as_usize();
        let total_patterns = metadata.enterprise_count + metadata.user_count;
        if pattern_id >= total_patterns {
            log::error!("Secret level not found for pattern ID {pattern_id}");
            continue;
        }
        let secret_level = if pattern_id < metadata.enterprise_count {
            SecretLevel::Enterprise
        } else {
            SecretLevel::User
        };

        secret_ranges.push((
            SecretRange {
                char_range: start_char..end_char,
                byte_range: start_byte..end_byte,
            },
            secret_level,
        ));
    }

    // Merge overlapping ranges, preserving the highest priority SecretLevel
    merge_sorted_ranges_with_levels(secret_ranges)
}

/// Merges overlapping ranges while preserving the highest priority SecretLevel
pub fn merge_sorted_ranges_with_levels(
    ranges: Vec<(SecretRange, SecretLevel)>,
) -> Vec<(SecretRange, SecretLevel)> {
    if ranges.is_empty() {
        return ranges;
    }

    let mut merged_ranges = vec![];
    let mut current_range = ranges[0].0.clone();
    let mut current_level = ranges[0].1;

    for (range, level) in ranges.into_iter().skip(1) {
        // We can merge based on character ranges since non-overlapping character ranges result in non-overlapping byte ranges.
        if range.char_range.start <= current_range.char_range.end {
            // Extend the current range to include the overlapping range.
            current_range.extend_range_end(&range);
            // Keep the highest priority level
            if level.priority() > current_level.priority() {
                current_level = level;
            }
        } else {
            // No overlap, push the current range and move to the next.
            merged_ranges.push((current_range, current_level));
            current_range = range;
            current_level = level;
        }
    }

    // Add the last range.
    merged_ranges.push((current_range, current_level));

    merged_ranges
}

/// R15: 自 app/ai/agent/redaction.rs 提升——就地遮蔽文本中全部检测到的 secrets。
pub fn redact_secrets(input: &mut String) {
    let mut secrets: Vec<_> = find_secrets_in_text(input)
        .into_iter()
        .map(|r| r.byte_range)
        .collect();
    // Replace from the end to preserve indices
    secrets.sort_by_key(|range| range.start);
    for range in secrets.into_iter().rev() {
        let replacement =
            SECRET_REDACTION_REPLACEMENT_CHARACTER.repeat(range.end.saturating_sub(range.start));
        input.replace_range(range.start..range.end, &replacement);
    }
}
