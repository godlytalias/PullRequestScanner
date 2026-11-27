use crate::error::CoreError;
use crate::process::{CancellationToken, ProcessResult, ProcessRunner};
use prs_model::{
    ChangedFile, ChangedFileStatus, CodeFoldRegion as ModelCodeFoldRegion, DiffBlock, DiffLine,
    DiffLineKind, DiffLinePair, ReviewSessionState,
};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

#[must_use]
pub fn parse_unified_diff(input: &str) -> Vec<DiffBlock> {
    let normalized = input.replace("\r\n", "\n");
    let mut blocks = Vec::new();
    let mut file_path = String::new();
    let mut previous_path = None;
    let mut lines = None::<Vec<DiffLine>>;
    let mut old_line = 0_usize;
    let mut new_line = 0_usize;

    let finish = |blocks: &mut Vec<DiffBlock>,
                  file_path: &str,
                  previous_path: &Option<String>,
                  lines: &mut Option<Vec<DiffLine>>| {
        if let Some(lines) = lines.take() {
            blocks.push(DiffBlock {
                file_path: file_path.to_string(),
                previous_path: previous_path.clone(),
                lines,
            });
        }
    };

    for line in normalized.split('\n') {
        if line.starts_with("diff --git ") {
            finish(&mut blocks, &file_path, &previous_path, &mut lines);
            let (old_path, new_path) = parse_diff_header(line);
            previous_path = Some(old_path);
            file_path = new_path;
            lines = Some(Vec::new());
            old_line = 0;
            new_line = 0;
            continue;
        }

        let Some(current_lines) = &mut lines else {
            continue;
        };

        if let Some(value) = line.strip_prefix("rename from ") {
            previous_path = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("rename to ") {
            file_path = value.trim().to_string();
            continue;
        }
        if line.starts_with("--- ") {
            continue;
        }
        if let Some(value) = line.strip_prefix("+++ ") {
            if value != "/dev/null" {
                file_path = strip_path_prefix(value);
            }
            continue;
        }

        if let Some(captures) = hunk_header().captures(line) {
            old_line = captures
                .name("old")
                .and_then(|value| value.as_str().parse().ok())
                .unwrap_or(0);
            new_line = captures
                .name("new")
                .and_then(|value| value.as_str().parse().ok())
                .unwrap_or(0);
            current_lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                file_path: file_path.clone(),
                text: line.to_string(),
                ..Default::default()
            });
            continue;
        }

        if line.starts_with("\\ No newline at end of file") {
            continue;
        }

        if old_line == 0 && new_line == 0 {
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                current_lines.push(DiffLine {
                    kind: DiffLineKind::Hunk,
                    file_path: file_path.clone(),
                    text: line.to_string(),
                    ..Default::default()
                });
            }
            continue;
        }

        if let Some(text) = line.strip_prefix('+') {
            current_lines.push(DiffLine {
                kind: DiffLineKind::Added,
                file_path: file_path.clone(),
                new_line_number: Some(new_line),
                text: text.to_string(),
                ..Default::default()
            });
            new_line += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            current_lines.push(DiffLine {
                kind: DiffLineKind::Deleted,
                file_path: file_path.clone(),
                old_line_number: Some(old_line),
                text: text.to_string(),
                ..Default::default()
            });
            old_line += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            current_lines.push(DiffLine {
                kind: DiffLineKind::Context,
                file_path: file_path.clone(),
                old_line_number: Some(old_line),
                new_line_number: Some(new_line),
                text: text.to_string(),
            });
            old_line += 1;
            new_line += 1;
        }
    }

    finish(&mut blocks, &file_path, &previous_path, &mut lines);
    blocks
}

#[must_use]
pub fn build_side_by_side(lines: &[DiffLine]) -> Vec<DiffLinePair> {
    let mut result = Vec::new();
    let mut deleted = Vec::new();
    let mut added = Vec::new();

    let flush =
        |deleted: &mut Vec<DiffLine>, added: &mut Vec<DiffLine>, result: &mut Vec<DiffLinePair>| {
            for index in 0..deleted.len().max(added.len()) {
                result.push(DiffLinePair {
                    left_line: deleted.get(index).cloned(),
                    right_line: added.get(index).cloned(),
                    hunk_line: None,
                });
            }
            deleted.clear();
            added.clear();
        };

    for line in lines {
        match line.kind {
            DiffLineKind::Deleted => deleted.push(line.clone()),
            DiffLineKind::Added => added.push(line.clone()),
            DiffLineKind::Hunk => {
                flush(&mut deleted, &mut added, &mut result);
                result.push(DiffLinePair {
                    left_line: None,
                    right_line: None,
                    hunk_line: Some(line.clone()),
                });
            }
            DiffLineKind::Context => {
                flush(&mut deleted, &mut added, &mut result);
                result.push(DiffLinePair {
                    left_line: Some(line.clone()),
                    right_line: Some(line.clone()),
                    hunk_line: None,
                });
            }
        }
    }
    flush(&mut deleted, &mut added, &mut result);
    result
}

#[must_use]
pub fn build_folds(lines: &[DiffLine]) -> Vec<CodeFoldRegion> {
    let mut regions = Vec::new();
    add_python_regions(lines, &mut regions);
    add_brace_regions(lines, &mut regions);
    regions.retain(|region| region.end_index > region.start_index);
    regions.sort_by_key(|region| region.start_index);
    regions.dedup_by(|right, left| {
        if right.start_index == left.start_index {
            if right.end_index > left.end_index {
                *left = right.clone();
            }
            true
        } else {
            false
        }
    });
    regions
}

pub struct GitDiffService {
    process_runner: ProcessRunner,
}

impl GitDiffService {
    #[must_use]
    pub const fn new(process_runner: ProcessRunner) -> Self {
        Self { process_runner }
    }

    pub async fn list_changed_files(
        &self,
        state: &ReviewSessionState,
    ) -> Result<Vec<ChangedFile>, CoreError> {
        self.list_changed_files_async(state, None, None, None).await
    }

    pub async fn list_changed_files_async(
        &self,
        state: &ReviewSessionState,
        base_commit_sha: Option<&str>,
        head_commit_sha: Option<&str>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Vec<ChangedFile>, CoreError> {
        let repository_path = require_repository(state)?;
        let arguments = vec![
            "diff".to_string(),
            "--name-status".to_string(),
            "-z".to_string(),
            "--find-renames=50%".to_string(),
            base_commit_sha
                .unwrap_or(&state.details.base_commit_sha)
                .to_string(),
            head_commit_sha
                .unwrap_or(&state.details.head_commit_sha)
                .to_string(),
        ];
        let result = self
            .run_git(repository_path, &arguments, cancellation_token)
            .await?;
        ensure_succeeded(&result, "list pull request files")?;
        Ok(parse_name_status(&result.standard_output))
    }

    pub async fn get_diff(
        &self,
        state: &ReviewSessionState,
        path_spec: &str,
        context_lines: i32,
        complete_file: bool,
    ) -> Result<Vec<DiffBlock>, CoreError> {
        self.get_diff_async(
            state,
            path_spec,
            context_lines,
            complete_file,
            None,
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_diff_async(
        &self,
        state: &ReviewSessionState,
        path_spec: &str,
        context_lines: i32,
        complete_file: bool,
        base_commit_sha: Option<&str>,
        head_commit_sha: Option<&str>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Vec<DiffBlock>, CoreError> {
        if context_lines < 0 {
            return Err(CoreError::operation(
                "context_lines must be greater than or equal to zero",
            ));
        }

        let repository_path = require_repository(state)?;
        let context = if complete_file {
            1_000_000
        } else {
            context_lines
        };
        let mut arguments = vec![
            "diff".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            "--find-renames=50%".to_string(),
            format!("--unified={context}"),
            base_commit_sha
                .unwrap_or(&state.details.base_commit_sha)
                .to_string(),
            head_commit_sha
                .unwrap_or(&state.details.head_commit_sha)
                .to_string(),
            "--".to_string(),
        ];
        let path_spec = if path_spec.trim().is_empty() || path_spec == "." {
            ".".to_string()
        } else {
            path_spec.replace('\\', "/")
        };
        arguments.push(path_spec);

        let result = self
            .run_git(repository_path, &arguments, cancellation_token)
            .await?;
        ensure_succeeded(&result, "read the pull request diff")?;
        Ok(parse_unified_diff(&result.standard_output))
    }

    pub async fn read_file(
        &self,
        state: &ReviewSessionState,
        commit: &str,
        path: &str,
    ) -> Result<String, CoreError> {
        self.read_file_async(state, commit, path, None).await
    }

    pub async fn read_file_async(
        &self,
        state: &ReviewSessionState,
        commit: &str,
        path: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, CoreError> {
        let repository_path = require_repository(state)?;
        let object = format!("{commit}:{}", path.replace('\\', "/"));
        let arguments = ["show".to_string(), "--no-textconv".to_string(), object];
        let result = self
            .run_git(repository_path, &arguments, cancellation_token)
            .await?;
        ensure_succeeded(&result, &format!("read '{path}' from commit {commit}"))?;
        Ok(result.standard_output)
    }

    #[must_use]
    pub fn process_runner(&self) -> &ProcessRunner {
        &self.process_runner
    }

    async fn run_git(
        &self,
        repository_path: &Path,
        arguments: &[String],
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ProcessResult, CoreError> {
        let mut git_arguments = Vec::with_capacity(arguments.len() + 2);
        git_arguments.push("-C".to_string());
        git_arguments.push(repository_path.to_string_lossy().into_owned());
        git_arguments.extend_from_slice(arguments);
        self.process_runner
            .run_async("git", git_arguments, None, None, cancellation_token)
            .await
            .map_err(map_git_process_error)
    }
}

impl Default for GitDiffService {
    fn default() -> Self {
        Self::new(ProcessRunner::new())
    }
}

#[must_use]
pub fn parse_name_status(value: &str) -> Vec<ChangedFile> {
    let fields = value.split('\0').collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while index < fields.len() && !fields[index].is_empty() {
        let status = fields[index];
        index += 1;
        let Some(code) = status.as_bytes().first().copied() else {
            continue;
        };
        if index >= fields.len() || fields[index].is_empty() {
            break;
        }

        if code == b'R' || code == b'C' {
            let previous_path = fields[index].to_string();
            index += 1;
            let path = fields
                .get(index)
                .copied()
                .unwrap_or(&previous_path)
                .to_string();
            if index < fields.len() {
                index += 1;
            }
            files.push(ChangedFile {
                path,
                previous_path: Some(previous_path),
                status: if code == b'R' {
                    ChangedFileStatus::Renamed
                } else {
                    ChangedFileStatus::Copied
                },
            });
        } else {
            files.push(ChangedFile {
                path: fields[index].to_string(),
                previous_path: None,
                status: match code {
                    b'A' => ChangedFileStatus::Added,
                    b'D' => ChangedFileStatus::Deleted,
                    b'T' => ChangedFileStatus::TypeChanged,
                    b'U' => ChangedFileStatus::Unmerged,
                    _ => ChangedFileStatus::Modified,
                },
            });
            index += 1;
        }
    }
    files
}

fn map_git_process_error(error: crate::process::ProcessError) -> CoreError {
    if error.is_not_found() {
        CoreError::operation(
            "Git is required to inspect the review, but the git executable was not found on PATH.",
        )
    } else {
        CoreError::Process(error)
    }
}

fn require_repository(state: &ReviewSessionState) -> Result<&Path, CoreError> {
    let path = state
        .local_repository_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            CoreError::operation(
                "The pull request repository is no longer available. Reopen the pull request to refresh it.",
            )
        })?;
    let path = Path::new(path);
    if !path.is_dir() {
        return Err(CoreError::operation(
            "The pull request repository is no longer available. Reopen the pull request to refresh it.",
        ));
    }
    Ok(path)
}

fn ensure_succeeded(result: &ProcessResult, operation: &str) -> Result<(), CoreError> {
    if result.succeeded() {
        return Ok(());
    }
    Err(CoreError::operation(format!(
        "Git could not {operation} (exit {}). {}",
        result.exit_code,
        result.standard_error.trim()
    )))
}

fn parse_diff_header(line: &str) -> (String, String) {
    let value = line.strip_prefix("diff --git ").unwrap_or_default();
    if let Some(captures) = diff_header().captures(value) {
        return (
            strip_path_prefix(&unquote(
                captures.name("old").map_or("", |value| value.as_str()),
            )),
            strip_path_prefix(&unquote(
                captures.name("new").map_or("", |value| value.as_str()),
            )),
        );
    }
    (strip_path_prefix(value), strip_path_prefix(value))
}

fn strip_path_prefix(path: &str) -> String {
    let trimmed = path.trim();
    let value = trimmed
        .strip_prefix("a/")
        .or_else(|| trimmed.strip_prefix("b/"))
        .unwrap_or(trimmed);
    unquote(value)
}

fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        value.to_string()
    }
}

fn hunk_header() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"^@@ -(?P<old>\d+)(?:,\d+)? \+(?P<new>\d+)(?:,\d+)? @@.*$")
            .expect("valid hunk regex")
    })
}

fn diff_header() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r#"^(?P<old>"(?:[^"\\]|\\.)*"|\S+)\s+(?P<new>"(?:[^"\\]|\\.)*"|\S+)$"#)
            .expect("valid diff header regex")
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFoldRegion {
    pub start_index: usize,
    pub end_index: usize,
    pub function_name: String,
}

impl CodeFoldRegion {
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.end_index.saturating_sub(self.start_index)
    }
}

impl From<CodeFoldRegion> for ModelCodeFoldRegion {
    fn from(value: CodeFoldRegion) -> Self {
        Self {
            start_index: value.start_index,
            end_index: value.end_index,
            function_name: value.function_name,
        }
    }
}

fn add_python_regions(lines: &[DiffLine], regions: &mut Vec<CodeFoldRegion>) {
    for (index, line) in lines.iter().enumerate() {
        let Some(rest) = line.text.trim_start().strip_prefix("def ") else {
            let Some(rest) = line.text.trim_start().strip_prefix("async def ") else {
                continue;
            };
            add_python_region(index, &line.text, rest, lines, regions);
            continue;
        };
        add_python_region(index, &line.text, rest, lines, regions);
    }
}

fn add_python_region(
    index: usize,
    line: &str,
    rest: &str,
    lines: &[DiffLine],
    regions: &mut Vec<CodeFoldRegion>,
) {
    let Some(open) = rest.find('(') else {
        return;
    };
    let name = rest[..open].trim().to_string();
    if name.is_empty() {
        return;
    }
    let indentation = line.len() - line.trim_start().len();
    let mut end = index;
    for (candidate, next) in lines.iter().enumerate().skip(index + 1) {
        if next.text.trim().is_empty() {
            continue;
        }
        let candidate_indent = next.text.len() - next.text.trim_start().len();
        if candidate_indent <= indentation {
            break;
        }
        end = candidate;
    }
    regions.push(CodeFoldRegion {
        start_index: index,
        end_index: end,
        function_name: name,
    });
}

fn add_brace_regions(lines: &[DiffLine], regions: &mut Vec<CodeFoldRegion>) {
    for (index, line) in lines.iter().enumerate() {
        let current = strip_strings_and_comments(&line.text);
        if !current.contains('{') {
            continue;
        }
        let start = index.saturating_sub(6);
        let mut header = lines[start..=index]
            .iter()
            .map(|line| strip_strings_and_comments(&line.text).trim().to_string())
            .collect::<Vec<_>>()
            .join(" ")
            .replace('\t', " ");
        if let (Some(semicolon), Some(paren)) = (header.find(';'), header.find('('))
            && semicolon > paren
        {
            header = header[header.rfind(';').unwrap_or(semicolon) + 1..].to_string();
        }
        let Some(captures) = function_regex().captures(&header) else {
            continue;
        };
        let name = captures.name("name").unwrap().as_str();
        if control_names().contains(&name) || is_type_declaration(&header) {
            continue;
        }
        let function_start = find_function_start(lines, start, index);
        let mut depth = brace_count(&current);
        let mut end = index;
        while depth > 0 && end + 1 < lines.len() {
            end += 1;
            depth += brace_count(&strip_strings_and_comments(&lines[end].text));
        }
        if depth == 0 {
            regions.push(CodeFoldRegion {
                start_index: function_start,
                end_index: end,
                function_name: name.to_string(),
            });
        }
    }
}

fn function_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(
            r"(?P<name>~?[A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)\s*\([^;{}]*\)\s*(?:const\b|override\b|final\b|noexcept\b|throws\b|->[^{};]+)*\s*\{$",
        )
        .expect("valid function regex")
    })
}

fn control_names() -> &'static [&'static str] {
    &[
        "if", "for", "foreach", "while", "switch", "catch", "sizeof", "typeof", "return", "new",
        "assert",
    ]
}

fn find_function_start(lines: &[DiffLine], lower_bound: usize, brace_index: usize) -> usize {
    let mut start = brace_index;
    for index in (lower_bound..brace_index).rev() {
        let text = strip_strings_and_comments(&lines[index].text);
        let trimmed = text.trim();
        if trimmed.is_empty()
            || trimmed.ends_with(';')
            || trimmed.ends_with('{')
            || trimmed.ends_with('}')
        {
            break;
        }
        start = index;
        if trimmed.contains('(') {
            break;
        }
    }
    start
}

fn is_type_declaration(header: &str) -> bool {
    let regex = type_declaration_regex();
    regex.is_match(header)
}

fn type_declaration_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\b(class|struct|enum|union|namespace|interface)\s+[A-Za-z_]\w*\s*(?:[:{])")
            .expect("valid type declaration regex")
    })
}

fn brace_count(line: &str) -> i32 {
    line.bytes().filter(|byte| *byte == b'{').count() as i32
        - line.bytes().filter(|byte| *byte == b'}').count() as i32
}

fn strip_strings_and_comments(value: &str) -> String {
    let mut output = value.chars().collect::<Vec<_>>();
    let mut in_string = false;
    let mut in_character = false;
    let mut escape = false;
    let mut index = 0;
    while index < output.len() {
        let current = output[index];
        if escape {
            output[index] = ' ';
            escape = false;
            index += 1;
            continue;
        }
        if !in_character
            && !in_string
            && current == '/'
            && index + 1 < output.len()
            && output[index + 1] == '/'
        {
            output[index..].fill(' ');
            break;
        }
        if current == '\\' && (in_string || in_character) {
            output[index] = ' ';
            escape = true;
            index += 1;
            continue;
        }
        if current == '"' && !in_character {
            in_string = !in_string;
            output[index] = ' ';
            index += 1;
            continue;
        }
        if current == '\'' && !in_string {
            in_character = !in_character;
            output[index] = ' ';
            index += 1;
            continue;
        }
        if in_string || in_character {
            output[index] = ' ';
        }
        index += 1;
    }
    output.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_old_and_new_line_numbers_across_hunk() {
        let diff = r#"diff --git a/src/Foo.cs b/src/Foo.cs
index 1111111..2222222 100644
--- a/src/Foo.cs
+++ b/src/Foo.cs
@@ -10,3 +10,4 @@ class Foo
 {
-    OldCall();
+    NewCall();
+    Extra();
 }
"#;
        let blocks = parse_unified_diff(diff);
        let block = &blocks[0];
        assert_eq!(block.file_path, "src/Foo.cs");
        let deleted = block
            .lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Deleted)
            .unwrap();
        assert_eq!(deleted.old_line_number, Some(11));
        assert_eq!(deleted.new_line_number, None);
        let added = block
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .map(|line| line.new_line_number)
            .collect::<Vec<_>>();
        assert_eq!(added, [Some(11), Some(12)]);
        assert_eq!(block.lines[1].old_line_number, Some(10));
        assert_eq!(block.lines[1].new_line_number, Some(10));
    }

    #[test]
    fn retains_paths_for_renamed_files() {
        let blocks = parse_unified_diff(
            r#"diff --git a/src/OldName.cs b/src/NewName.cs
similarity index 95%
rename from src/OldName.cs
rename to src/NewName.cs
--- a/src/OldName.cs
+++ b/src/NewName.cs
@@ -1 +1 @@
-old
+new
"#,
        );
        assert_eq!(blocks[0].previous_path.as_deref(), Some("src/OldName.cs"));
        assert_eq!(blocks[0].file_path, "src/NewName.cs");
    }

    #[test]
    fn parses_added_and_deleted_file_line_ranges() {
        let added = parse_unified_diff(
            r#"diff --git a/src/New.cs b/src/New.cs
new file mode 100644
--- /dev/null
+++ b/src/New.cs
@@ -0,0 +1,2 @@
+class New
+{
"#,
        );
        assert_eq!(
            added[0]
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Added)
                .map(|line| line.new_line_number)
                .collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );

        let deleted = parse_unified_diff(
            r#"diff --git a/src/Old.cs b/src/Old.cs
deleted file mode 100644
--- a/src/Old.cs
+++ /dev/null
@@ -1,2 +0,0 @@
-class Old
-{
"#,
        );
        assert_eq!(
            deleted[0]
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Deleted)
                .map(|line| line.old_line_number)
                .collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
    }

    #[test]
    fn parses_nul_separated_name_status_including_rename_and_copy() {
        let value = "A\0src/added.cs\0M\0src/modified.cs\0D\0src/deleted.cs\0T\0src/type\0U\0src/conflict\0R100\0src/old.cs\0src/new.cs\0C075\0src/source.cs\0src/copied.cs\0";
        let files = parse_name_status(value);
        assert_eq!(
            files.iter().map(|file| file.status).collect::<Vec<_>>(),
            [
                ChangedFileStatus::Added,
                ChangedFileStatus::Modified,
                ChangedFileStatus::Deleted,
                ChangedFileStatus::TypeChanged,
                ChangedFileStatus::Unmerged,
                ChangedFileStatus::Renamed,
                ChangedFileStatus::Copied,
            ]
        );
        assert_eq!(files[5].previous_path.as_deref(), Some("src/old.cs"));
        assert_eq!(files[5].path, "src/new.cs");
        assert_eq!(files[6].previous_path.as_deref(), Some("src/source.cs"));
        assert_eq!(files[6].path, "src/copied.cs");
    }

    #[test]
    fn pairs_changes_and_keeps_hunks_full_width() {
        let hunk = DiffLine {
            kind: DiffLineKind::Hunk,
            text: "@@ -1,2 +1,3 @@".to_string(),
            ..Default::default()
        };
        let lines = [
            DiffLine {
                kind: DiffLineKind::Context,
                old_line_number: Some(10),
                new_line_number: Some(10),
                text: "before".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Deleted,
                old_line_number: Some(11),
                text: "old one".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Deleted,
                old_line_number: Some(12),
                text: "old two".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Added,
                new_line_number: Some(11),
                text: "new one".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Context,
                old_line_number: Some(13),
                new_line_number: Some(12),
                text: "after".to_string(),
                ..Default::default()
            },
            hunk.clone(),
        ];
        let rows = build_side_by_side(&lines);

        assert_eq!(rows[0].left_line.as_ref().unwrap().text, "before");
        assert_eq!(rows[0].left_line, rows[0].right_line);
        assert_eq!(rows[1].left_line.as_ref().unwrap().text, "old one");
        assert_eq!(rows[1].right_line.as_ref().unwrap().text, "new one");
        assert_eq!(rows[2].left_line.as_ref().unwrap().text, "old two");
        assert!(rows[2].right_line.is_none());
        assert_eq!(rows[3].right_line.as_ref().unwrap().text, "after");
        assert_eq!(rows[4].hunk_line.as_ref(), Some(&hunk));
        assert!(rows[4].is_hunk());
    }

    #[test]
    fn builds_brace_and_python_folds() {
        let brace_lines = [
            DiffLine {
                text: "int Transform(int value)".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "{".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "    return value + 1;".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "}".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "int Next() { return 2; }".to_string(),
                ..Default::default()
            },
        ];
        let transform = &build_folds(&brace_lines)[0];
        assert_eq!(transform.function_name, "Transform");
        assert_eq!(transform.start_index, 0);
        assert_eq!(transform.end_index, 3);

        let python_lines = [
            DiffLine {
                text: "async def transform(value):".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "    if value:".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "        return value + 1".to_string(),
                ..Default::default()
            },
            DiffLine {
                text: "next_value = 0".to_string(),
                ..Default::default()
            },
        ];
        let python = &build_folds(&python_lines)[0];
        assert_eq!(python.function_name, "transform");
        assert_eq!(python.start_index, 0);
        assert_eq!(python.end_index, 2);
    }
}
