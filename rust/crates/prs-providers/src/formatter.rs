use prs_model::{ReviewComment, ReviewCommentSide, ReviewProvider};

pub struct ReviewCommentFormatter;

impl ReviewCommentFormatter {
    #[must_use]
    pub fn create_published_body(provider: ReviewProvider, comment: &ReviewComment) -> String {
        let mut body = comment.body.trim().to_string();
        if !comment.selected_text.trim().is_empty() {
            append_separator(&mut body);
            body.push_str("Selected code:\n");
            for selected_line in comment.selected_text.replace("\r\n", "\n").split('\n') {
                body.push_str("> ");
                body.push_str(selected_line);
                body.push('\n');
            }
        }

        if comment.suggested_replacement.trim().is_empty() {
            return body.trim_end().to_string();
        }

        let replacement_line = Self::build_suggested_line(comment);
        append_separator(&mut body);
        if provider == ReviewProvider::GitHub && comment.side == ReviewCommentSide::Right {
            body.push_str("```suggestion\n");
            body.push_str(&replacement_line);
            body.push('\n');
            body.push_str("```");
        } else {
            body.push_str("Suggested replacement:\n```diff\n");
            if !comment.source_line_text.is_empty() {
                body.push_str("- ");
                body.push_str(&comment.source_line_text);
                body.push('\n');
            }
            body.push_str("+ ");
            body.push_str(&replacement_line);
            body.push('\n');
            body.push_str("```");
        }

        body.trim_end().to_string()
    }

    #[must_use]
    pub fn build_suggested_line(comment: &ReviewComment) -> String {
        let Some(selection_start) = comment.selection_start else {
            return comment.suggested_replacement.clone();
        };

        let source: Vec<u16> = comment.source_line_text.encode_utf16().collect();
        if selection_start > source.len() || comment.selection_length == 0 {
            return comment.suggested_replacement.clone();
        }

        let selection_length = comment
            .selection_length
            .min(source.len().saturating_sub(selection_start));
        let prefix = String::from_utf16_lossy(&source[..selection_start]);
        let suffix = String::from_utf16_lossy(&source[selection_start + selection_length..]);
        format!("{prefix}{}{suffix}", comment.suggested_replacement)
    }
}

fn append_separator(body: &mut String) {
    if !body.is_empty() && !body.ends_with("\n\n") {
        body.push_str("\n\n");
    }
}
