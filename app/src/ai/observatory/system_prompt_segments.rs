//! SystemPrompt block 详情的标记感知分段器（T11）。
//!
//! harness system prompt（Claude Code / OpenAI / opencode 等）由"结构标记 +
//! 正文"交替构成：markdown ATX 标题（`# Harness`）、独立 XML-ish 标签行
//! （`<env>`…`</env>`、`<available_skills>`、`<example>`）、方括号节标题
//! （`[Environment]`）。观测台详情页把这些标记识别为段边界，逐段折叠展示。
//!
//! 不变量（单测覆盖）：
//! - **无损**：所有段 `text` 顺序拼接恒等于输入原文。
//! - 纯函数：无 IO、无 DB、无 UI 依赖，可独立单测。

/// 段起始标记的种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentMarkerKind {
    /// 独立 XML-ish 标签行，如 `<env>` / `<available_skills>`。
    XmlTag,
    /// markdown ATX 标题，如 `# Harness` / `## Editing Approach`。
    MarkdownHeader,
    /// 方括号节标题，如 `[Environment]`。
    BracketHeader,
}

/// system prompt 的一个结构段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemPromptSegment {
    /// 标记名（XML 标签名或标题文本）；无标记的自由文本段为 `None`。
    pub marker: Option<String>,
    /// 标记种类；自由文本段为 `None`。
    pub kind: Option<SegmentMarkerKind>,
    /// 折叠态摘要：正文首条非空行截断（无正文则空串）。
    pub summary: String,
    /// 段原文（含标记行本身），展开态渲染用。
    pub text: String,
    /// 段起始行号（1-based）。
    pub line_start: usize,
    /// 段行数（含标记行）。
    pub line_count: usize,
}

/// 折叠态摘要的最大字符数。
const SUMMARY_MAX_CHARS: usize = 100;

/// 标签名合法字符（XML Name 简化版：字母开头 + 字母数字 `_.:-`）。
fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ':' || c == '-'
}

/// 解析独立 XML 标签行（trim 后整行恰好是一个标签，允许属性与自闭合）。
/// 返回 `(名字, 是否闭合标签)`。
fn parse_xml_tag_line(trimmed: &str) -> Option<(String, bool)> {
    let body = trimmed.strip_prefix('<')?.strip_suffix('>')?;
    if let Some(name) = body.strip_prefix('/') {
        // </tag>
        let name = name.trim();
        if name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && name.chars().all(is_tag_char)
        {
            return Some((name.to_string(), true));
        }
        return None;
    }
    // <tag attr="..."> / <tag/>：名字截至首个空白或 `/`。
    let name: &str = body
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("");
    if name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && name.chars().all(is_tag_char)
    {
        Some((name.to_string(), false))
    } else {
        None
    }
}

/// 解析 markdown ATX 标题行（`#`×1..6 + 空白 + 非空内容），返回标题文本。
fn parse_atx_header_line(trimmed: &str) -> Option<String> {
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    let title = rest.trim();
    if title.is_empty() || rest.starts_with('#') {
        return None;
    }
    Some(title.to_string())
}

/// 解析方括号节标题行（整行 `[Name]`，Name 以字母开头，字母数字/空白/`_-`）。
fn parse_bracket_header_line(trimmed: &str) -> Option<String> {
    let body = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    if !body.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if body
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-')
    {
        Some(body.to_string())
    } else {
        None
    }
}

/// 把原文按结构标记分段。空/全空白输入返回空 Vec。
///
/// 规则：
/// - 段边界只出现在标记行（或 XML 段的匹配闭合行之后）；非标记行永远归属
///   最近一段，首个标记之前的正文构成无标记的 preamble 段。
/// - XML 标签段内同名标签深度计数，直到匹配 `</tag>` 才收段；段内的
///   markdown 标题/子标签不再切分（repo 地图类正文常含 `#`/`<file>` 行）。
/// - 代码围栏（``` 开关）内的行一律不当标记。
pub fn segment_system_prompt(content: &str) -> Vec<SystemPromptSegment> {
    if content.trim().is_empty() {
        return Vec::new();
    }

    let mut segments: Vec<SystemPromptSegment> = Vec::new();
    // 当前段标记：None = 尚未开段（收集 preamble）。
    let mut cur_marker: Option<(String, SegmentMarkerKind)> = None;
    // XML 段状态：同名开标签深度（非 XML 段恒 None）。
    let mut xml_depth: Option<(String, usize)> = None;
    // 当前段首行 index（0-based）。
    let mut seg_start: usize = 0;
    let mut in_fence = false;
    // 行缓冲：段文本按行切好，收段时拼接（join 保换行）。
    let mut lines: Vec<&str> = Vec::new();

    let flush = |segments: &mut Vec<SystemPromptSegment>,
                     lines: &mut Vec<&str>,
                     marker: &Option<(String, SegmentMarkerKind)>,
                     start: usize| {
        if lines.is_empty() {
            return;
        }
        let text = lines.join("\n");
        // 摘要用正文：去掉首行标记；XML 段再去掉末行同名闭合。
        let body: Vec<&str> = match marker {
            Some((_, SegmentMarkerKind::XmlTag)) => {
                let mut body_lines = lines[1..].to_vec();
                if let Some(last) = body_lines.last() {
                    if let Some((name, true)) = parse_xml_tag_line(last.trim()) {
                        if Some(&name) == marker.as_ref().map(|(n, _)| n) {
                            body_lines.pop();
                        }
                    }
                }
                body_lines
            }
            Some(_) => lines[1..].to_vec(),
            None => lines.clone(),
        };
        segments.push(SystemPromptSegment {
            marker: marker.as_ref().map(|(n, _)| n.clone()),
            kind: marker.as_ref().map(|(_, k)| *k),
            summary: summarize(&body),
            text,
            line_start: start + 1,
            line_count: lines.len(),
        });
        lines.clear();
    };

    for (idx, raw_line) in content.split('\n').enumerate() {
        let trimmed = raw_line.trim();

        // 代码围栏开关：围栏内不是标记。
        if trimmed.starts_with("```") {
            lines.push(raw_line);
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(raw_line);
            continue;
        }

        // XML 段内：只认同名开/闭标签（深度计数），其余行归段。
        if let Some((name, depth)) = xml_depth.as_mut() {
            if let Some((tname, is_close)) = parse_xml_tag_line(trimmed) {
                if tname == *name {
                    if is_close {
                        *depth -= 1;
                    } else {
                        *depth += 1;
                    }
                    lines.push(raw_line);
                    if *depth == 0 {
                        flush(&mut segments, &mut lines, &cur_marker, seg_start);
                        cur_marker = None;
                        xml_depth = None;
                    }
                    continue;
                }
            }
            lines.push(raw_line);
            continue;
        }

        // 非段内：标记行开新段。
        let marker = if let Some((name, is_close)) = parse_xml_tag_line(trimmed) {
            if is_close {
                None // 孤立闭合标签：当普通正文行。
            } else {
                Some((name, SegmentMarkerKind::XmlTag))
            }
        } else if let Some(title) = parse_atx_header_line(trimmed) {
            Some((title, SegmentMarkerKind::MarkdownHeader))
        } else if let Some(title) = parse_bracket_header_line(trimmed) {
            Some((title, SegmentMarkerKind::BracketHeader))
        } else {
            None
        };

        match marker {
            Some((name, kind)) => {
                // 收束 pending 段。纯空白 pending（XML 闭合后/文档开头的
                // 空行）不单独成段：有前段则并入前段文本尾部（保无损），
                // 无前段则留作新段前缀。
                let pending_blank_only =
                    cur_marker.is_none() && lines.iter().all(|l| l.trim().is_empty());
                if pending_blank_only {
                    if let Some(prev) = segments.last_mut() {
                        let pending_text = lines.join("\n");
                        prev.text.push('\n');
                        prev.text.push_str(&pending_text);
                        prev.line_count += lines.len();
                        lines.clear();
                    }
                    // segments 为空：保留 lines 作新段前缀（seg_start 不动）。
                } else {
                    flush(&mut segments, &mut lines, &cur_marker, seg_start);
                    seg_start = idx;
                }
                if kind == SegmentMarkerKind::XmlTag {
                    xml_depth = Some((name.clone(), 1));
                }
                cur_marker = Some((name, kind));
                lines.push(raw_line);
            }
            None => {
                // 尚未开段（preamble / XML 闭合后的自由文本）记录首行位置。
                if cur_marker.is_none() && lines.is_empty() {
                    seg_start = idx;
                }
                lines.push(raw_line);
            }
        }
    }

    // 兜底收尾：XML 闭合后的纯空白尾行并入前段，再收未闭合段/preamble。
    if cur_marker.is_none() && lines.iter().all(|l| l.trim().is_empty()) {
        if let Some(prev) = segments.last_mut() {
            let pending_text = lines.join("\n");
            prev.text.push('\n');
            prev.text.push_str(&pending_text);
            prev.line_count += lines.len();
            lines.clear();
        }
    }
    flush(&mut segments, &mut lines, &cur_marker, seg_start);

    // 无损修正：段文本按各自行 join，边界换行（split 丢掉的行终止符）
    // 归前一段——除最后一段外统一补上，拼接即原文。
    if segments.len() > 1 {
        let last = segments.len() - 1;
        for seg in &mut segments[..last] {
            seg.text.push('\n');
        }
    }

    segments
}

/// 摘要：首条非空、非围栏行，截断至 [`SUMMARY_MAX_CHARS`] 字符。
fn summarize(body_lines: &[&str]) -> String {
    for line in body_lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with("```") {
            continue;
        }
        let mut s: String = t.chars().take(SUMMARY_MAX_CHARS).collect();
        if t.chars().count() > SUMMARY_MAX_CHARS {
            s.push('…');
        }
        return s;
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无损不变量：段文本顺序拼接 == 原文。
    fn assert_lossless(input: &str, segments: &[SystemPromptSegment]) {
        assert_eq!(
            segments.iter().map(|s| s.text.as_str()).collect::<String>(),
            input,
            "segments must reassemble to the original content"
        );
    }


    /// 样本 A：Claude Code（Anthropic）风格 —— preamble + ATX 标题 +
    /// 围栏内代码 + XML example 块。
    const ANTHROPIC_SAMPLE: &str = "\
You are Claude Code, Anthropic's official CLI for Claude.

You are an interactive agent that helps users with software engineering tasks.

# Harness
 - Text you output outside of tool use is displayed to the user.
 - `<system-reminder>` tags in messages are injected by the harness.

# Memory

You have a persistent file-based memory at `/home/yy/.claude/memory/`:

```markdown
---
name: sample
---
<the fact; not a marker>
```

# Environment
You have been invoked in the following environment:
 - Primary working directory: /home/yy/warpdotdev/zap
 - Platform: linux

<example>
user: Where are errors handled?
assistant: In `connectToServer` in src/services/process.ts:712.
</example>
";

    /// 样本 B：OpenAI/GPT 风格 —— 多级 ATX 标题，无 XML 标签。
    const OPENAI_SAMPLE: &str = "\
You are Zap, the best coding agent on the planet.

## Editing Approach
- Take an iterative approach
- Save changes frequently

### `commentary` channel
Use commentary for progress updates.

## Response channels
Different channels map to different surfaces.
";

    /// 样本 C：opencode 风格 —— env 块 / available_skills 嵌套子标签。
    const OPENCODE_SAMPLE: &str = "\
You are opencode, a coding agent.

<env>
Working directory: /home/yy/warpdotdev/zap
Is directory a git repo: Yes
Platform: linux
</env>

Free text between blocks.

<available_skills>
<skill>
<name>commit</name>
<description>Create git commits</description>
</skill>
<skill>
<name>review</name>
</skill>
</available_skills>

[Environment]
Shell: bash
";

    #[test]
    fn anthropic_sample_segments() {
        let segs = segment_system_prompt(ANTHROPIC_SAMPLE);
        assert_lossless(ANTHROPIC_SAMPLE, &segs);

        // 逐段断言：preamble + 3 个 ATX 标题段 + example XML 段。
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(
            names,
            vec![
                None,
                Some("Harness"),
                Some("Memory"),
                Some("Environment"),
                Some("example"),
            ]
        );
        let kinds: Vec<Option<SegmentMarkerKind>> = segs.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                None,
                Some(SegmentMarkerKind::MarkdownHeader),
                Some(SegmentMarkerKind::MarkdownHeader),
                Some(SegmentMarkerKind::MarkdownHeader),
                Some(SegmentMarkerKind::XmlTag),
            ]
        );

        // preamble 摘要取首条非空行。
        assert_eq!(
            segs[0].summary,
            "You are Claude Code, Anthropic's official CLI for Claude."
        );

        // Memory 段含围栏，围栏内 <the fact; not a marker> 不切分。
        assert!(segs[2].text.contains("```markdown"));
        assert!(segs[2].text.contains("<the fact; not a marker>"));

        // example 段：XML 收段含闭合行，正文摘要去掉首尾标签。
        assert!(segs[4].text.starts_with("<example>"));
        assert!(segs[4].text.trim_end().ends_with("</example>"));
        assert_eq!(segs[4].summary, "user: Where are errors handled?");

        // 行号 1-based 连续覆盖。
        let mut expect_start = 1;
        for s in &segs {
            assert_eq!(s.line_start, expect_start);
            expect_start += s.line_count;
        }
    }

    #[test]
    fn openai_sample_segments() {
        let segs = segment_system_prompt(OPENAI_SAMPLE);
        assert_lossless(OPENAI_SAMPLE, &segs);
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(
            names,
            vec![
                None,
                Some("Editing Approach"),
                Some("`commentary` channel"),
                Some("Response channels"),
            ]
        );
        // 除 preamble 外全部 markdown 标题段。
        assert!(segs
            .iter()
            .skip(1)
            .all(|s| s.kind == Some(SegmentMarkerKind::MarkdownHeader)));
        assert_eq!(segs[1].summary, "- Take an iterative approach");
    }

    #[test]
    fn opencode_env_and_nested_skills() {
        let segs = segment_system_prompt(OPENCODE_SAMPLE);
        assert_lossless(OPENCODE_SAMPLE, &segs);
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(
            names,
            vec![
                None,
                Some("env"),
                None,
                Some("available_skills"),
                Some("Environment"),
            ]
        );
        // env 段正文摘要跳过标签行。
        assert_eq!(
            segs[1].summary,
            "Working directory: /home/yy/warpdotdev/zap"
        );
        // available_skills 内嵌 <skill> 子标签，同名深度计数不切分；
        // 整段到 </available_skills> 收束。
        assert!(segs[3].text.contains("<name>commit</name>"));
        assert!(segs[3].text.contains("<skill>"));
        assert!(segs[3].text.trim_end().ends_with("</available_skills>"));
        // 方括号标题段。
        assert_eq!(segs[4].kind, Some(SegmentMarkerKind::BracketHeader));
        assert_eq!(segs[4].summary, "Shell: bash");
    }

    #[test]
    fn no_markers_yields_single_free_segment() {
        let input = "just plain text\nsecond line\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].marker, None);
        assert_eq!(segs[0].kind, None);
        assert_eq!(segs[0].summary, "just plain text");
    }

    #[test]
    fn unclosed_xml_tag_runs_to_eof() {
        let input = "<repo_map>\nsrc/main.rs\n# not a header inside xml\n# Harness\nafter\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        // 未闭合 <repo_map>：XML 段内不认标题，整段吞到 EOF（64 KiB 截断
        // 场景下等价于"剩下全是它"）。
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(names, vec![Some("repo_map")]);
        assert!(segs[0].text.contains("# not a header inside xml"));
        assert!(segs[0].text.trim_end().ends_with("after"));
    }

    #[test]
    fn closed_xml_resumes_at_next_marker() {
        // 闭合后的自由文本独立成段，下一个标记正常开新段。
        let input = "<env>\ndir: /tmp\n</env>\nafter close text\n# Next\nbody\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(names, vec![Some("env"), None, Some("Next")]);
        assert_eq!(segs[1].summary, "after close text");
        assert_eq!(segs[1].line_start, 4);
    }

    #[test]
    fn fenced_lines_are_never_markers() {
        let input = "# Title\n```text\n# Fake Header\n<fake_tag>\n```\n# Real\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(names, vec![Some("Title"), Some("Real")]);
        assert!(segs[0].text.contains("# Fake Header"));
        assert!(segs[0].text.contains("<fake_tag>"));
    }

    #[test]
    fn inline_tag_mentions_are_not_markers() {
        // 行内提及的标签（前后有正文）不是独立标签行。
        let input = "Prefer the dedicated tools. `<system-reminder>` tags are injected.\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].marker, None);
    }

    #[test]
    fn summary_truncates_long_lines() {
        let long = "x".repeat(200);
        let input = format!("# Section\n{long}\n");
        let segs = segment_system_prompt(&input);
        assert_eq!(segs[0].summary.chars().count(), 101); // 100 chars + …
        assert!(segs[0].summary.ends_with('…'));
    }

    #[test]
    fn empty_and_blank_inputs() {
        assert!(segment_system_prompt("").is_empty());
        assert!(segment_system_prompt("   \n\n  ").is_empty());
    }

    #[test]
    fn tag_line_parsing_edge_cases() {
        // 属性/自闭合标签行是标记；`<>`、`<1abc>`、非整行不是。
        assert_eq!(
            parse_xml_tag_line("<env mode=\"full\">"),
            Some(("env".to_string(), false))
        );
        assert_eq!(parse_xml_tag_line("<br/>"), Some(("br".to_string(), false)));
        assert_eq!(parse_xml_tag_line("</env>"), Some(("env".to_string(), true)));
        assert_eq!(parse_xml_tag_line("<>"), None);
        assert_eq!(parse_xml_tag_line("<1abc>"), None);
        assert_eq!(parse_xml_tag_line("a<b>"), None);
        assert_eq!(parse_xml_tag_line("<a> text"), None);
    }

    #[test]
    fn orphan_close_tag_is_plain_text() {
        let input = "text\n</orphan>\nmore\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].marker, None);
        assert!(segs[0].text.contains("</orphan>"));
    }

    #[test]
    fn xml_segment_ignores_headers_inside() {
        // repo 地图类正文：XML 段内的 `#` 行与子标签都不切分。
        let input =
            "<repo_map>\n# file: src/main.rs\n<file>src/lib.rs</file>\nfunc foo() {}\n</repo_map>\n";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].marker.as_deref(), Some("repo_map"));
        assert_eq!(segs[0].summary, "# file: src/main.rs");
    }

    #[test]
    fn real_claude_code_prompt_shape() {
        // 真实 Claude Code system prompt（截取自 harness_blocks.db）形状回归：
        // 计费头 + intro 段 + 多个 `# 标题` 段。
        let input = "\
x-anthropic-billing-header: cc_version=2.1.179.efd; cc_entrypoint=cli; cch=c7d24;
You are Claude Code, Anthropic's official CLI for Claude.

# Harness
 - Tools run behind a user-selected permission mode.

# Context management
When the conversation grows long.
";
        let segs = segment_system_prompt(input);
        assert_lossless(input, &segs);
        let names: Vec<Option<&str>> = segs.iter().map(|s| s.marker.as_deref()).collect();
        assert_eq!(
            names,
            vec![None, Some("Harness"), Some("Context management")]
        );
        // preamble 摘要取首条非空行（计费头）。
        assert!(segs[0].summary.starts_with("x-anthropic-billing-header:"));
    }
}
