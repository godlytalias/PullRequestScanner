use crate::error::CoreError;
use crate::process::{CancellationToken, ProcessRunner};
use prs_model::{ReviewSessionState, SymbolLocation};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolCandidate {
    pub name: String,
    pub display_name: String,
    pub is_likely_external: bool,
    pub column_index: usize,
}

impl SymbolCandidate {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        display_name: impl Into<String>,
        is_likely_external: bool,
        column_index: usize,
    ) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            is_likely_external,
            column_index,
        }
    }
}

pub struct SymbolCandidateExtractor;

impl SymbolCandidateExtractor {
    #[must_use]
    pub fn extract_candidates(code: &str) -> Vec<SymbolCandidate> {
        let stripped = strip_comments_and_strings(code);
        let mut candidates = Vec::new();
        for captures in call_regex().captures_iter(&stripped) {
            let matched = captures
                .name("qualified")
                .expect("qualified capture is part of the regex");
            let previous = stripped[..matched.start()].chars().next_back();
            if previous.is_some_and(|character| {
                character == '.' || character == '_' || character.is_alphanumeric()
            }) {
                continue;
            }

            let display_name = matched.as_str().to_string();
            let name = simple_name(&display_name).to_string();
            if keywords()
                .iter()
                .any(|keyword| keyword.eq_ignore_ascii_case(&name))
                || candidates
                    .iter()
                    .any(|candidate: &SymbolCandidate| candidate.display_name == display_name)
            {
                continue;
            }

            let root = display_name
                .split("::")
                .flat_map(|part| part.split('.'))
                .next()
                .unwrap_or_default()
                .to_string();
            candidates.push(SymbolCandidate::new(
                name,
                display_name,
                external_roots()
                    .iter()
                    .any(|external| external.eq_ignore_ascii_case(&root)),
                stripped[..matched.start()].encode_utf16().count(),
            ));
        }
        candidates
    }

    #[must_use]
    pub fn extract(code: &str) -> Vec<String> {
        Self::extract_candidates(code)
            .into_iter()
            .map(|candidate| candidate.name)
            .collect()
    }
}

#[must_use]
pub fn extract_symbol_candidates(code: &str) -> Vec<SymbolCandidate> {
    SymbolCandidateExtractor::extract_candidates(code)
}

pub struct SymbolNavigationService {
    process_runner: ProcessRunner,
}

impl SymbolNavigationService {
    #[must_use]
    pub const fn new(process_runner: ProcessRunner) -> Self {
        Self { process_runner }
    }

    pub async fn find_definition(
        &self,
        state: &ReviewSessionState,
        symbol: &str,
        preferred_path: &str,
    ) -> Result<Option<SymbolLocation>, CoreError> {
        self.find_definition_async(state, symbol, preferred_path, None, None, None)
            .await
    }

    pub async fn find_definition_async(
        &self,
        state: &ReviewSessionState,
        symbol: &str,
        preferred_path: &str,
        commit: Option<&str>,
        source_line_number: Option<usize>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<SymbolLocation>, CoreError> {
        let repository_path = state
            .local_repository_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| CoreError::operation("The pull request repository is not available."))?;
        let commit = commit.unwrap_or(&state.details.head_commit_sha);
        let preferred_path = preferred_path.replace('\\', "/");
        let directory = Path::new(&preferred_path)
            .parent()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let mut candidates = Vec::new();

        self.add_matches(
            &mut candidates,
            repository_path,
            symbol,
            commit,
            std::slice::from_ref(&preferred_path),
            cancellation_token,
        )
        .await?;
        if !has_strong_definition(&candidates, &preferred_path, source_line_number)
            && !directory.is_empty()
        {
            self.add_matches(
                &mut candidates,
                repository_path,
                symbol,
                commit,
                &[format!(":(glob){directory}/**/*")],
                cancellation_token,
            )
            .await?;
        }
        if !has_strong_definition(&candidates, &preferred_path, source_line_number) {
            self.add_matches(
                &mut candidates,
                repository_path,
                symbol,
                commit,
                &extension_path_specs(&preferred_path),
                cancellation_token,
            )
            .await?;
        }

        let mut selected = None::<SymbolLocation>;
        for candidate in candidates
            .into_iter()
            .map(|candidate| score(candidate, &preferred_path))
            .filter(|candidate| candidate.score >= 120)
            .filter(|candidate| {
                source_line_number.is_none()
                    || !candidate.file_path.eq_ignore_ascii_case(&preferred_path)
                    || candidate.line_number != source_line_number.unwrap()
            })
        {
            if selected
                .as_ref()
                .is_none_or(|selected| candidate.score > selected.score)
            {
                selected = Some(candidate);
            }
        }
        Ok(selected)
    }

    #[must_use]
    pub fn process_runner(&self) -> &ProcessRunner {
        &self.process_runner
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_matches(
        &self,
        candidates: &mut Vec<SymbolLocation>,
        repository_path: &str,
        symbol: &str,
        commit: &str,
        path_specs: &[String],
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        let mut arguments = vec![
            "-C".to_string(),
            repository_path.to_string(),
            "grep".to_string(),
            "--color=never".to_string(),
            "--line-number".to_string(),
            "--fixed-strings".to_string(),
            "--word-regexp".to_string(),
            symbol.to_string(),
            commit.to_string(),
            "--".to_string(),
        ];
        arguments.extend_from_slice(path_specs);
        let result = self
            .process_runner
            .run_async("git", arguments, None, None, cancellation_token)
            .await
            .map_err(|error| {
                if error.is_not_found() {
                    CoreError::operation(
                        "Git is required to search symbols, but the git executable was not found on PATH.",
                    )
                } else {
                    CoreError::Process(error)
                }
            })?;
        if result.exit_code == 1 {
            return Ok(());
        }
        if !result.succeeded() {
            return Err(CoreError::operation(format!(
                "Git could not search for '{symbol}' (exit {}). {}",
                result.exit_code,
                result.standard_error.trim()
            )));
        }

        let prefix = format!("{commit}:");
        for line in result.standard_output.lines() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let value = line.strip_prefix(&prefix).unwrap_or(line);
            let Some(line_separator) = value.find(':') else {
                continue;
            };
            let number_start = line_separator + 1;
            let Some(relative_number_separator) = value[number_start..].find(':') else {
                continue;
            };
            let number_separator = number_start + relative_number_separator;
            let Ok(line_number) = value[number_start..number_separator].parse::<usize>() else {
                continue;
            };
            let candidate = SymbolLocation {
                symbol: symbol.to_string(),
                file_path: value[..line_separator].to_string(),
                line_number,
                line_text: value[number_separator + 1..].to_string(),
                score: 0,
            };
            if !candidates.iter().any(|existing| {
                existing
                    .file_path
                    .eq_ignore_ascii_case(&candidate.file_path)
                    && existing.line_number == candidate.line_number
            }) {
                candidates.push(candidate);
            }
        }
        Ok(())
    }
}

impl Default for SymbolNavigationService {
    fn default() -> Self {
        Self::new(ProcessRunner::new())
    }
}

fn simple_name(display_name: &str) -> &str {
    let colon_separator = display_name.rfind("::");
    let dot_separator = display_name.rfind('.');
    match (colon_separator, dot_separator) {
        (Some(colon), Some(dot)) if colon > dot => &display_name[colon + 2..],
        (_, Some(dot)) => &display_name[dot + 1..],
        (Some(colon), None) => &display_name[colon + 2..],
        (None, None) => display_name,
    }
}

fn has_strong_definition(
    candidates: &[SymbolLocation],
    preferred_path: &str,
    source_line_number: Option<usize>,
) -> bool {
    candidates
        .iter()
        .cloned()
        .map(|candidate| score(candidate, preferred_path))
        .any(|candidate| {
            candidate.score >= 120
                && (source_line_number.is_none()
                    || !candidate.file_path.eq_ignore_ascii_case(preferred_path)
                    || candidate.line_number != source_line_number.unwrap())
        })
}

fn score(mut location: SymbolLocation, preferred_path: &str) -> SymbolLocation {
    location.score = definition_score(
        &location.line_text,
        &location.symbol,
        Path::new(&location.file_path),
    );
    if location.file_path.eq_ignore_ascii_case(preferred_path) {
        location.score += 20;
    }
    location
}

fn definition_score(line: &str, symbol: &str, file_path: &Path) -> i32 {
    let escaped = regex::escape(symbol);
    let strong_patterns = [
        format!(r"^\s*(?:class|struct|interface|enum|record|type)\s+(?:\w+::)*{escaped}\b"),
        format!(r"^\s*(?:async\s+)?def\s+{escaped}\s*\("),
        format!(
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+)*fn\s+{escaped}\s*(?:<[^>]+>)?\s*\("
        ),
        format!(r"^\s*func\s+(?:\([^)]*\)\s*)?{escaped}\s*\("),
        format!(
            r"^\s*(?:export\s+)?(?:declare\s+)?(?:async\s+)?function\s+(?:\w+::)*{escaped}\s*(?:<[^>]+>)?\s*\("
        ),
        format!(r"^\s*#\s*define\s+{escaped}\s*(?:\(|\b)"),
    ];
    if strong_patterns
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .any(|pattern| pattern.is_match(line))
    {
        return 160;
    }

    let c_like_pattern = format!(
        r"^\s*(?P<prefix>(?:(?:public|private|protected|internal|static|virtual|override|abstract|async|extern|sealed|partial|final|native|constexpr|consteval|constinit|inline|explicit|friend|[\w.:<>,?*&~\[\]]+)\s+)+)(?:[\w:<>~]+\s*::\s*)?{escaped}(?:<[^;(){{}}]+>)?\s*\("
    );
    if let Ok(c_like) = Regex::new(&c_like_pattern)
        && let Some(captures) = c_like.captures(line)
        && !contains_control_keyword(captures.name("prefix").unwrap().as_str())
    {
        return 160;
    }

    let method_pattern = format!(
        r"^\s*(?:public|private|protected|internal|static|async|override|virtual|abstract|sealed|partial|\s)*{escaped}\s*\([^;]*\)\s*(?::[^\{{=;]+)?(?:\{{|=>)"
    );
    if Regex::new(&method_pattern).is_ok_and(|pattern| pattern.is_match(line)) {
        return 160;
    }

    let extension = file_path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    if [".js", ".jsx", ".ts", ".tsx"]
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    {
        let variable_pattern = format!(r"^\s*(?:export\s+)?(?:const|let|var)\s+{escaped}\s*=");
        if Regex::new(&variable_pattern).is_ok_and(|pattern| pattern.is_match(line)) {
            return 140;
        }
    }

    0
}

fn contains_control_keyword(prefix: &str) -> bool {
    prefix.split_whitespace().any(|word| {
        keywords()
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case(word))
    })
}

fn extension_path_specs(path: &str) -> Vec<String> {
    let specs: &[&str] = match Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "cs" => &[":(glob)**/*.cs"],
        "c" | "cc" | "cpp" | "cxx" => &[
            ":(glob)**/*.[ch]",
            ":(glob)**/*.cc",
            ":(glob)**/*.cpp",
            ":(glob)**/*.cxx",
            ":(glob)**/*.h",
            ":(glob)**/*.hpp",
        ],
        "h" | "hh" | "hpp" | "hxx" => &[
            ":(glob)**/*.h",
            ":(glob)**/*.hpp",
            ":(glob)**/*.cpp",
            ":(glob)**/*.cc",
            ":(glob)**/*.c",
        ],
        "js" | "jsx" | "ts" | "tsx" => &[
            ":(glob)**/*.js",
            ":(glob)**/*.jsx",
            ":(glob)**/*.ts",
            ":(glob)**/*.tsx",
        ],
        "py" => &[":(glob)**/*.py"],
        "go" => &[":(glob)**/*.go"],
        "rs" => &[":(glob)**/*.rs"],
        "java" | "kt" => &[":(glob)**/*.java", ":(glob)**/*.kt"],
        _ => &["."],
    };
    specs.iter().map(|spec| (*spec).to_string()).collect()
}

fn strip_comments_and_strings(code: &str) -> String {
    let text = code.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(code.len());
    let mut in_string = false;
    let mut in_character = false;
    let mut escaped = false;
    let mut index = 0;
    while index < text.len() {
        let current = text[index];
        if in_string || in_character {
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if in_string && current == '"' {
                in_string = false;
            } else if in_character && current == '\'' {
                in_character = false;
            }
            push_mask(&mut output, current);
            index += 1;
            continue;
        }
        if current == '"' {
            in_string = true;
            push_mask(&mut output, current);
        } else if current == '\'' {
            in_character = true;
            push_mask(&mut output, current);
        } else if current == '/' && text.get(index + 1) == Some(&'/') {
            for character in &text[index..] {
                push_mask(&mut output, *character);
            }
            break;
        } else if current == '/' && text.get(index + 1) == Some(&'*') {
            let mut end = index + 2;
            while end + 1 < text.len() && !(text[end] == '*' && text[end + 1] == '/') {
                end += 1;
            }
            let length = if end + 1 < text.len() {
                end + 2 - index
            } else {
                text.len() - index
            };
            for character in &text[index..index + length] {
                push_mask(&mut output, *character);
            }
            index += length.saturating_sub(1);
        } else {
            output.push(current);
        }
        index += 1;
    }
    output
}

fn push_mask(output: &mut String, character: char) {
    for _ in 0..character.len_utf16() {
        output.push(' ');
    }
}

fn call_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"(?P<qualified>(?:[A-Za-z_]\w*(?:::|\.))*[A-Za-z_]\w*)\s*\(")
            .expect("valid call regex")
    })
}

fn keywords() -> &'static [&'static str] {
    &[
        "if", "for", "foreach", "while", "switch", "catch", "using", "new", "typeof", "sizeof",
        "nameof", "lock", "return", "else", "with", "assert",
    ]
}

fn external_roots() -> &'static [&'static str] {
    &["std", "__std", "System"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_function_candidates_and_skips_control_keywords() {
        assert_eq!(
            SymbolCandidateExtractor::extract(
                "if (CanReview(item)) ReviewItem(item); else RetryAsync();"
            ),
            ["CanReview", "ReviewItem", "RetryAsync"]
        );
    }

    #[test]
    fn preserves_qualification_and_marks_standard_library_calls_external() {
        let candidates = SymbolCandidateExtractor::extract_candidates(
            "return std::clamp(value, low, high) + LocalHelper(value);",
        );
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "clamp");
        assert_eq!(candidates[0].display_name, "std::clamp");
        assert_eq!(candidates[0].column_index, 7);
        assert!(candidates[0].is_likely_external);
        assert_eq!(candidates[1].name, "LocalHelper");
        assert!(!candidates[1].is_likely_external);
    }

    #[test]
    fn reports_symbol_columns_in_utf16_units() {
        let candidate = &SymbolCandidateExtractor::extract_candidates("😀 LocalHelper();")[0];
        assert_eq!(candidate.column_index, 3);
    }

    #[test]
    fn ignores_comments_and_strings() {
        assert!(SymbolCandidateExtractor::extract(r#"// Example header;"#).is_empty());
        assert_eq!(
            SymbolCandidateExtractor::extract(
                r#"string text = "QuotedCall()"; ASSERT(Check()); ActualCall(); /* Hidden() */"#
            ),
            ["Check", "ActualCall"]
        );
    }
}
