use prs_model::{MarkdownBlock, MarkdownBlockKind, MarkdownInlineKind, MarkdownInlineRun};
use regex::Regex;
use std::sync::OnceLock;
use url::Url;

#[must_use]
pub fn parse(markdown: &str) -> Vec<MarkdownBlock> {
    let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized.split('\n').collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        let trimmed = line.trim_start();

        if let Some(language) = trimmed.strip_prefix("```") {
            let start = index + 1;
            let mut code = Vec::new();
            index += 1;
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                code.push(lines[index]);
                index += 1;
            }
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Code,
                text: code.join("\n"),
                source_line_number: start + 1,
                source_line_count: code.len().max(1),
                language: Some(language.trim().to_string()),
                ..Default::default()
            });
            if index < lines.len() {
                index += 1;
            }
            continue;
        }

        let heading = trimmed
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if (1..=6).contains(&heading) && trimmed.as_bytes().get(heading) == Some(&b' ') {
            let text = &trimmed[heading + 1..];
            blocks.push(MarkdownBlock {
                kind: heading_kind(heading),
                text: format_inline(text),
                source_line_number: index + 1,
                source_line_count: 1,
                inline_runs: Some(parse_inline_runs(text)),
                ..Default::default()
            });
            index += 1;
            continue;
        }

        if matches!(trimmed, "---" | "***" | "___") {
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::HorizontalRule,
                source_line_number: index + 1,
                source_line_count: 1,
                ..Default::default()
            });
            index += 1;
            continue;
        }

        if let Some(captures) = task_regex().captures(trimmed) {
            let checked = captures
                .name("checked")
                .map(|value| value.as_str())
                .unwrap_or(" ")
                .eq_ignore_ascii_case("x");
            let text = captures.name("text").map_or("", |value| value.as_str());
            blocks.push(MarkdownBlock {
                kind: if checked {
                    MarkdownBlockKind::TaskChecked
                } else {
                    MarkdownBlockKind::TaskUnchecked
                },
                text: format_inline(text),
                source_line_number: index + 1,
                source_line_count: 1,
                list_marker: Some(if checked { "\u{e73e}" } else { "\u{e739}" }.into()),
                inline_runs: Some(parse_inline_runs(text)),
                ..Default::default()
            });
            index += 1;
            continue;
        }

        if let Some(text) = bullet_item(trimmed) {
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Bullet,
                text: format_inline(text),
                source_line_number: index + 1,
                source_line_count: 1,
                list_marker: Some("•".into()),
                inline_runs: Some(parse_inline_runs(text)),
                ..Default::default()
            });
            index += 1;
            continue;
        }

        if trimmed.contains('|')
            && index + 1 < lines.len()
            && table_separator().is_match(lines[index + 1].trim())
        {
            let start = index;
            let mut table = Vec::new();
            while index < lines.len() && lines[index].trim().contains('|') {
                table.push(lines[index].trim());
                index += 1;
            }
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Table,
                text: table.join("\n"),
                source_line_number: start + 1,
                source_line_count: (index - start).max(1),
                ..Default::default()
            });
            continue;
        }

        if let Some(captures) = numbered_regex().captures(trimmed) {
            let marker = captures.name("marker").map_or("", |value| value.as_str());
            let text = &trimmed[marker.len()..].trim_start();
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Numbered,
                text: format_inline(text),
                source_line_number: index + 1,
                source_line_count: 1,
                list_marker: Some(marker.trim().to_string()),
                inline_runs: Some(parse_inline_runs(text)),
                ..Default::default()
            });
            index += 1;
            continue;
        }

        if let Some(text) = trimmed.strip_prefix('>') {
            let text = text.trim_start();
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Quote,
                text: format_inline(text),
                source_line_number: index + 1,
                source_line_count: 1,
                inline_runs: Some(parse_inline_runs(text)),
                ..Default::default()
            });
            index += 1;
            continue;
        }

        let start = index;
        let mut paragraph = Vec::new();
        loop {
            paragraph.push(lines[index].trim());
            index += 1;
            if index >= lines.len()
                || lines[index].trim().is_empty()
                || !is_paragraph_line(lines[index])
            {
                break;
            }
        }
        let text = paragraph.join(" ");
        blocks.push(MarkdownBlock {
            kind: MarkdownBlockKind::Paragraph,
            text: format_inline(&text),
            source_line_number: start + 1,
            source_line_count: (index - start).max(1),
            inline_runs: Some(parse_inline_runs(&text)),
            ..Default::default()
        });
    }
    blocks
}

#[must_use]
pub fn to_plain_text(markdown: &str) -> String {
    let value = markdown
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let value = html_comment_regex().replace_all(&value, "");
    let value = html_tag_regex().replace_all(&value, "");
    format_inline(&html_unescape(&value))
}

fn parse_inline_runs(value: &str) -> Vec<MarkdownInlineRun> {
    let mut runs = Vec::new();
    let mut position = 0;
    for capture in inline_regex().captures_iter(value) {
        let full = capture.get(0).unwrap();
        if full.start() > position {
            runs.push(MarkdownInlineRun {
                kind: MarkdownInlineKind::Text,
                text: html_unescape(&value[position..full.start()]),
                navigate_uri: None,
            });
        }
        if let Some(text) = capture.name("bold_text") {
            runs.push(MarkdownInlineRun {
                kind: MarkdownInlineKind::Bold,
                text: html_unescape(text.as_str()),
                navigate_uri: None,
            });
        } else if let Some(text) = capture.name("code_text") {
            runs.push(MarkdownInlineRun {
                kind: MarkdownInlineKind::Code,
                text: text.as_str().to_string(),
                navigate_uri: None,
            });
        } else if let (Some(label), Some(uri)) =
            (capture.name("link_text"), capture.name("link_url"))
        {
            if let Ok(uri) = Url::parse(uri.as_str()) {
                runs.push(MarkdownInlineRun {
                    kind: MarkdownInlineKind::Link,
                    text: html_unescape(label.as_str()),
                    navigate_uri: Some(uri),
                });
            } else {
                runs.push(MarkdownInlineRun {
                    kind: MarkdownInlineKind::Text,
                    text: html_unescape(full.as_str()),
                    navigate_uri: None,
                });
            }
        }
        position = full.end();
    }
    if position < value.len() {
        runs.push(MarkdownInlineRun {
            kind: MarkdownInlineKind::Text,
            text: html_unescape(&value[position..]),
            navigate_uri: None,
        });
    }
    if runs.is_empty() {
        runs.push(MarkdownInlineRun {
            kind: MarkdownInlineKind::Text,
            text: String::new(),
            navigate_uri: None,
        });
    }
    runs
}

fn heading_kind(value: usize) -> MarkdownBlockKind {
    match value {
        1 => MarkdownBlockKind::Heading1,
        2 => MarkdownBlockKind::Heading2,
        3 => MarkdownBlockKind::Heading3,
        4 => MarkdownBlockKind::Heading4,
        5 => MarkdownBlockKind::Heading5,
        _ => MarkdownBlockKind::Heading6,
    }
}

fn bullet_item(value: &str) -> Option<&str> {
    let mut chars = value.char_indices();
    let (_, marker) = chars.next()?;
    if !matches!(marker, '-' | '+' | '*') {
        return None;
    }
    value[marker.len_utf8()..]
        .strip_prefix(char::is_whitespace)
        .map(str::trim)
}

fn is_paragraph_line(value: &str) -> bool {
    let trimmed = value.trim_start();
    !trimmed.starts_with("```")
        && !trimmed.starts_with('#')
        && bullet_item(trimmed).is_none()
        && !numbered_regex().is_match(trimmed)
        && !trimmed.starts_with('>')
        && !matches!(trimmed, "---" | "***" | "___")
}

fn format_inline(value: &str) -> String {
    let value = image_regex().replace_all(value, "$name");
    let value = link_regex().replace_all(&value, "$label ($url)");
    value
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn task_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"^[-+*]\s+\[(?P<checked>[ xX])\]\s+(?P<text>.+)$").expect("valid task regex")
    })
}

fn numbered_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"^(?P<marker>\d+[.)]\s+)").expect("valid numbered regex"))
}

fn table_separator() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r#"^\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)+\|?$"#).expect("valid table separator regex")
    })
}

fn inline_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(
            r"(?s)(?:\*\*(?P<bold_text>.+?)\*\*)|(?:`(?P<code_text>[^`]+?)`)|(?:\[(?P<link_text>[^\]]+)\]\((?P<link_url>[^)]+)\))",
        )
        .expect("valid inline regex")
    })
}

fn image_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"!\[(?P<name>[^\]]*)\]\([^)]+\)").expect("valid image regex"))
}

fn link_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\[(?P<label>[^\]]+)\]\((?P<url>[^)]+)\)").expect("valid link regex")
    })
}

fn html_comment_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").expect("valid comment regex"))
}

fn html_tag_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"<[^>]+>").expect("valid HTML tag regex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extended_blocks() {
        let blocks = parse(
            "#### Details\n- [x] done\n- [ ] pending\n\n| Build | Result |\n|---|---|\n| x64 | Pass |",
        );
        assert_eq!(blocks[0].kind, MarkdownBlockKind::Heading4);
        assert_eq!(blocks[1].kind, MarkdownBlockKind::TaskChecked);
        assert_eq!(blocks[2].kind, MarkdownBlockKind::TaskUnchecked);
        assert_eq!(blocks[3].kind, MarkdownBlockKind::Table);
        assert!(blocks[3].text.contains("| x64 | Pass |"));
    }

    #[test]
    fn preserves_inline_runs() {
        let block = parse("Use **bold**, `code`, and [docs](https://example.test/docs).")
            .pop()
            .unwrap();
        let runs = block.inline_runs.unwrap();
        assert!(
            runs.iter()
                .any(|run| run.kind == MarkdownInlineKind::Bold && run.text == "bold")
        );
        assert!(
            runs.iter()
                .any(|run| run.kind == MarkdownInlineKind::Code && run.text == "code")
        );
        assert!(runs.iter().any(|run| run.kind == MarkdownInlineKind::Link));
    }
}
