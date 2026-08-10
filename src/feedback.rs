//! Fail-closed deterministic repair for exact automation suggestions.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

const MAX_PATCH_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    pub actor_login: String,
    pub actor_type: String,
    pub is_automation: bool,
    pub body: String,
    pub path: Option<String>,
    pub start_line: Option<u32>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairDecision {
    Preview,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairDisposition {
    HumanFeedback,
    RejectedActor,
    Previewed,
    Applied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackReply<'a> {
    Applied { path: &'a str, commit: &'a str },
    Rejected,
}

pub fn render_feedback_reply(reply: FeedbackReply<'_>) -> String {
    match reply {
        FeedbackReply::Applied { path, commit } => format!(
            "PiTools applied this approved automation suggestion to `{}` and pushed commit `{}` after configured validation passed. The review thread is being resolved.",
            safe_reply_fragment(path),
            safe_reply_fragment(commit)
        ),
        FeedbackReply::Rejected => "PiTools rejected this automation suggestion after deterministic validation failed. No branch change was accepted. The review thread is being resolved; human follow-up remains required.".into(),
    }
}

fn safe_reply_fragment(value: &str) -> String {
    let fragment: String = value
        .chars()
        .filter(|character| !matches!(character, '`' | '\r' | '\n'))
        .take(128)
        .collect();
    if fragment.is_empty() {
        "(unavailable)".into()
    } else {
        fragment
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairOutcome {
    pub disposition: RepairDisposition,
    pub path: Option<String>,
    pub resolution_eligible: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FeedbackError {
    #[error("suggested change is malformed: {0}")]
    MalformedSuggestion(String),
    #[error("unified patch is malformed: {0}")]
    MalformedPatch(String),
    #[error("patch path is invalid: {0}")]
    InvalidPath(String),
    #[error("patch does not apply exactly: {0}")]
    StalePatch(String),
    #[error("repository I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
struct ParsedPatch {
    path: String,
    hunks: Vec<Hunk>,
}

#[derive(Debug)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_count: usize,
    lines: Vec<HunkLine>,
}

#[derive(Debug)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

pub fn repair_feedback(
    repository_root: &Path,
    automation_actor_allowlist: &[String],
    feedback: &Feedback,
    decision: RepairDecision,
) -> Result<RepairOutcome, FeedbackError> {
    if !feedback.is_automation || feedback.actor_type != "Bot" {
        return Ok(non_mutating(RepairDisposition::HumanFeedback));
    }
    if !automation_actor_allowlist
        .iter()
        .any(|actor| actor == &feedback.actor_login)
    {
        return Ok(non_mutating(RepairDisposition::RejectedActor));
    }

    let prepared = prepare_repair(repository_root, feedback)?;

    match decision {
        RepairDecision::Preview => Ok(RepairOutcome {
            disposition: RepairDisposition::Previewed,
            path: Some(prepared.path),
            resolution_eligible: false,
        }),
        RepairDecision::Apply => {
            // Recheck immediately before the sole mutation to reject a replaced symlink.
            let target = checked_target(repository_root, &prepared.path)?;
            let current = fs::read_to_string(&target)?;
            if current != prepared.original {
                return Err(FeedbackError::StalePatch(
                    "target changed after validation".into(),
                ));
            }
            fs::write(&target, &prepared.repaired)?;
            if fs::read_to_string(&target)? != prepared.repaired {
                return Err(FeedbackError::StalePatch(
                    "target readback differs from the validated repair".into(),
                ));
            }
            Ok(RepairOutcome {
                disposition: RepairDisposition::Applied,
                path: Some(prepared.path),
                resolution_eligible: true,
            })
        }
    }
}

struct PreparedRepair {
    path: String,
    original: String,
    repaired: String,
}

fn prepare_repair(
    repository_root: &Path,
    feedback: &Feedback,
) -> Result<PreparedRepair, FeedbackError> {
    if let Ok(suggestion) = extract_exact_suggestion(&feedback.body)
        && suggestion.starts_with("--- a/")
    {
        let patch = parse_unified_patch(suggestion)?;
        let target = checked_target(repository_root, &patch.path)?;
        let original = fs::read_to_string(&target)?;
        let repaired = apply_hunks(&original, &patch.hunks)?;
        return Ok(PreparedRepair {
            path: patch.path,
            original,
            repaired,
        });
    }

    let suggestion = extract_inline_suggestion(&feedback.body)?;
    let path = feedback
        .path
        .clone()
        .ok_or_else(|| FeedbackError::MalformedSuggestion("review path is missing".into()))?;
    let line = feedback
        .line
        .ok_or_else(|| FeedbackError::MalformedSuggestion("review line is missing".into()))?;
    let start_line = feedback.start_line.unwrap_or(line);
    if line == 0 || start_line == 0 || start_line > line {
        return Err(FeedbackError::MalformedSuggestion(
            "review line range is invalid".into(),
        ));
    }
    let target = checked_target(repository_root, &path)?;
    let original = fs::read_to_string(&target)?;
    let repaired = apply_inline_suggestion(&original, start_line, line, suggestion)?;
    Ok(PreparedRepair {
        path,
        original,
        repaired,
    })
}

fn non_mutating(disposition: RepairDisposition) -> RepairOutcome {
    RepairOutcome {
        disposition,
        path: None,
        resolution_eligible: false,
    }
}

fn extract_exact_suggestion(body: &str) -> Result<&str, FeedbackError> {
    if body.len() > MAX_PATCH_BYTES || body.contains('\r') {
        return Err(FeedbackError::MalformedSuggestion(
            "unsupported size or line endings".into(),
        ));
    }
    let body = body.strip_suffix('\n').unwrap_or(body);
    let suggestion = body
        .strip_prefix("```suggestion\n")
        .and_then(|value| value.strip_suffix("\n```"))
        .ok_or_else(|| {
            FeedbackError::MalformedSuggestion(
                "expected exactly one suggestion fence and no surrounding text".into(),
            )
        })?;
    if suggestion.is_empty() || suggestion.contains("```") {
        return Err(FeedbackError::MalformedSuggestion(
            "empty or ambiguous suggestion fence".into(),
        ));
    }
    Ok(suggestion)
}

fn extract_inline_suggestion(body: &str) -> Result<&str, FeedbackError> {
    if body.len() > MAX_PATCH_BYTES || body.contains('\r') {
        return Err(FeedbackError::MalformedSuggestion(
            "unsupported size or line endings".into(),
        ));
    }
    let marker = "```suggestion\n";
    if body.match_indices(marker).count() != 1 {
        return Err(FeedbackError::MalformedSuggestion(
            "expected exactly one suggestion fence".into(),
        ));
    }
    let start = body
        .find(marker)
        .expect("the unique suggestion marker was counted");
    let content_start = start + marker.len();
    let closing = body[content_start..]
        .find("\n```")
        .map(|offset| content_start + offset)
        .ok_or_else(|| {
            FeedbackError::MalformedSuggestion("unterminated suggestion fence".into())
        })?;
    let suggestion = &body[content_start..closing];
    if suggestion.contains("```") {
        return Err(FeedbackError::MalformedSuggestion(
            "suggestion contains a nested code fence".into(),
        ));
    }
    Ok(suggestion)
}

fn apply_inline_suggestion(
    original: &str,
    start_line: u32,
    end_line: u32,
    suggestion: &str,
) -> Result<String, FeedbackError> {
    if original.contains('\r') || suggestion.contains('\r') {
        return Err(FeedbackError::StalePatch(
            "line-ending normalization is not supported".into(),
        ));
    }
    let has_trailing_newline = original.ends_with('\n');
    let source = original.strip_suffix('\n').unwrap_or(original);
    let mut lines: Vec<String> = if source.is_empty() {
        Vec::new()
    } else {
        source.split('\n').map(str::to_owned).collect()
    };
    let start = usize::try_from(start_line - 1).expect("u32 fits usize");
    let end = usize::try_from(end_line).expect("u32 fits usize");
    if end > lines.len() || start >= end {
        return Err(FeedbackError::StalePatch(
            "review line range is outside the current file".into(),
        ));
    }
    let replacement: Vec<String> = if suggestion.is_empty() {
        Vec::new()
    } else {
        suggestion.split('\n').map(str::to_owned).collect()
    };
    lines.splice(start..end, replacement);
    let mut repaired = lines.join("\n");
    if has_trailing_newline && !lines.is_empty() {
        repaired.push('\n');
    }
    Ok(repaired)
}

fn parse_unified_patch(source: &str) -> Result<ParsedPatch, FeedbackError> {
    let mut lines = source.split('\n').peekable();
    let old_path = lines
        .next()
        .and_then(|line| line.strip_prefix("--- a/"))
        .ok_or_else(|| FeedbackError::MalformedPatch("missing old-file header".into()))?;
    let new_path = lines
        .next()
        .and_then(|line| line.strip_prefix("+++ b/"))
        .ok_or_else(|| FeedbackError::MalformedPatch("missing new-file header".into()))?;
    if old_path != new_path {
        return Err(FeedbackError::MalformedPatch(
            "old and new paths must match".into(),
        ));
    }
    validate_relative_path(old_path)?;

    let mut hunks = Vec::new();
    while let Some(header) = lines.next() {
        if header.is_empty() {
            return Err(FeedbackError::MalformedPatch(
                "unexpected blank line between hunks".into(),
            ));
        }
        let (old_start, old_count, new_count) = parse_hunk_header(header)?;
        let mut hunk_lines = Vec::new();
        while lines.peek().is_some_and(|line| !line.starts_with("@@")) {
            let line = lines.next().expect("peeked hunk line");
            let parsed = match line.as_bytes().first() {
                Some(b' ') => HunkLine::Context(line[1..].to_string()),
                Some(b'-') => HunkLine::Remove(line[1..].to_string()),
                Some(b'+') => HunkLine::Add(line[1..].to_string()),
                _ => {
                    return Err(FeedbackError::MalformedPatch(
                        "hunk line lacks a valid prefix".into(),
                    ));
                }
            };
            hunk_lines.push(parsed);
        }
        let actual_old = hunk_lines
            .iter()
            .filter(|line| matches!(line, HunkLine::Context(_) | HunkLine::Remove(_)))
            .count();
        let actual_new = hunk_lines
            .iter()
            .filter(|line| matches!(line, HunkLine::Context(_) | HunkLine::Add(_)))
            .count();
        if actual_old != old_count || actual_new != new_count {
            return Err(FeedbackError::MalformedPatch(
                "hunk header line counts do not match its body".into(),
            ));
        }
        hunks.push(Hunk {
            old_start,
            old_count,
            new_count,
            lines: hunk_lines,
        });
    }
    if hunks.is_empty() {
        return Err(FeedbackError::MalformedPatch("patch has no hunks".into()));
    }
    Ok(ParsedPatch {
        path: old_path.to_string(),
        hunks,
    })
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize, usize), FeedbackError> {
    let rest = header
        .strip_prefix("@@ ")
        .ok_or_else(|| FeedbackError::MalformedPatch("invalid hunk header".into()))?;
    let (ranges, _) = rest
        .split_once(" @@")
        .ok_or_else(|| FeedbackError::MalformedPatch("unterminated hunk header".into()))?;
    let mut ranges = ranges.split(' ');
    let old = ranges
        .next()
        .and_then(|range| range.strip_prefix('-'))
        .ok_or_else(|| FeedbackError::MalformedPatch("missing old hunk range".into()))?;
    let new = ranges
        .next()
        .and_then(|range| range.strip_prefix('+'))
        .ok_or_else(|| FeedbackError::MalformedPatch("missing new hunk range".into()))?;
    if ranges.next().is_some() {
        return Err(FeedbackError::MalformedPatch(
            "unexpected hunk range fields".into(),
        ));
    }
    let (old_start, old_count) = parse_range(old)?;
    let (_, new_count) = parse_range(new)?;
    if old_start == 0 && old_count != 0 {
        return Err(FeedbackError::MalformedPatch(
            "old line numbers are one-based".into(),
        ));
    }
    Ok((old_start, old_count, new_count))
}

fn parse_range(range: &str) -> Result<(usize, usize), FeedbackError> {
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (start, count),
        None => (range, "1"),
    };
    let start = start
        .parse()
        .map_err(|_| FeedbackError::MalformedPatch("invalid hunk start".into()))?;
    let count = count
        .parse()
        .map_err(|_| FeedbackError::MalformedPatch("invalid hunk count".into()))?;
    Ok((start, count))
}

fn validate_relative_path(value: &str) -> Result<(), FeedbackError> {
    if value.is_empty() || value.contains('\\') || value.contains('\0') {
        return Err(FeedbackError::InvalidPath(value.into()));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(FeedbackError::InvalidPath(value.into()));
    }
    let normalized = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if normalized != value {
        return Err(FeedbackError::InvalidPath(value.into()));
    }
    Ok(())
}

fn checked_target(repository_root: &Path, relative: &str) -> Result<PathBuf, FeedbackError> {
    validate_relative_path(relative)?;
    let root = fs::canonicalize(repository_root)?;
    let mut target = root.clone();
    for component in Path::new(relative).components() {
        target.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&target)?;
        if metadata.file_type().is_symlink() {
            return Err(FeedbackError::InvalidPath(relative.into()));
        }
    }
    if !fs::metadata(&target)?.is_file() {
        return Err(FeedbackError::InvalidPath(relative.into()));
    }
    let canonical = fs::canonicalize(&target)?;
    if !canonical.starts_with(&root) || canonical != target {
        return Err(FeedbackError::InvalidPath(relative.into()));
    }
    Ok(target)
}

fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, FeedbackError> {
    let source: Vec<&str> = original.split('\n').collect();
    let mut result = Vec::new();
    let mut cursor = 0usize;

    for hunk in hunks {
        let start = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start.saturating_sub(1)
        };
        if start < cursor || start > source.len() {
            return Err(FeedbackError::StalePatch(
                "hunks overlap or address lines outside the file".into(),
            ));
        }
        result.extend(source[cursor..start].iter().map(|line| (*line).to_string()));
        let mut source_index = start;
        let mut produced = 0usize;
        for line in &hunk.lines {
            match line {
                HunkLine::Context(expected) => {
                    require_exact_line(&source, source_index, expected)?;
                    result.push(expected.clone());
                    source_index += 1;
                    produced += 1;
                }
                HunkLine::Remove(expected) => {
                    require_exact_line(&source, source_index, expected)?;
                    source_index += 1;
                }
                HunkLine::Add(value) => {
                    result.push(value.clone());
                    produced += 1;
                }
            }
        }
        if produced != hunk.new_count {
            return Err(FeedbackError::MalformedPatch(
                "new hunk count changed during application".into(),
            ));
        }
        cursor = source_index;
    }
    result.extend(source[cursor..].iter().map(|line| (*line).to_string()));
    Ok(result.join("\n"))
}

fn require_exact_line(source: &[&str], index: usize, expected: &str) -> Result<(), FeedbackError> {
    if source.get(index).copied() != Some(expected) {
        return Err(FeedbackError::StalePatch(format!(
            "source line {} does not match the hunk",
            index + 1
        )));
    }
    Ok(())
}
