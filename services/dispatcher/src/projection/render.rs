//! The escaping status-led renderer: an [`OperationEvent`] in, deterministic Telegram HTML out.
//!
//! Determinism is load-bearing — the rendered bytes feed the job's `content_hash`, which is how
//! identical re-renders become detectable no-ops downstream. Display branches on STATUS only,
//! never on stage vocabulary: the stage is producer display text of no contract standing, so it
//! is appended verbatim-but-escaped and nothing else.

use crate::projection::event::{OperationEvent, OperationStatus, SafeLine};

/// The hard character cap on a rendered body. Telegram's message limit is 4096; the margin
/// absorbs the HTML tags the renderer wraps around the body.
const BODY_CAP_CHARS: usize = 3900;

/// How many error or warning lines survive into one render before the "+N more" line.
const MAX_LINES_PER_KIND: usize = 3;

/// Render one event as Telegram HTML (`<b>` bold only).
///
/// Layout: the status display line leads (bold), an optional `— NN%` follows, then the escaped
/// stage, then up to three error and three warning lines as `- (code) message`, then a
/// `+N more` fold line when a kind overflows. The whole body is truncated to
/// [`BODY_CAP_CHARS`] characters at a character boundary.
///
/// The same event always renders to byte-identical output.
#[must_use]
pub fn render(event: &OperationEvent) -> String {
    let mut out = String::new();
    out.push_str("<b>");
    out.push_str(display_line(event.status));
    out.push_str("</b>");
    if let Some(percent) = event.progress_percent {
        out.push_str(" \u{2014} ");
        out.push_str(&percent.to_string());
        out.push('%');
    }
    if let Some(stage) = &event.stage {
        out.push('\n');
        out.push_str(&escape_html(stage));
    }
    append_lines(&mut out, &event.errors);
    append_lines(&mut out, &event.warnings);
    truncate_chars(&out, BODY_CAP_CHARS)
}

/// The status-led display vocabulary. Stage text never reaches this decision.
const fn display_line(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Accepted => "Accepted",
        OperationStatus::Queued => "Queued",
        OperationStatus::Running => "In progress",
        OperationStatus::Succeeded => "Completed",
        OperationStatus::PartiallySucceeded => "Completed with warnings",
        OperationStatus::Failed => "Failed",
        OperationStatus::Cancelled => "Cancelled",
    }
}

/// Append up to [`MAX_LINES_PER_KIND`] lines as `- (code) message`, folding the rest into a
/// `+N more` line. Both code and message go through escaping: the code is contract-constrained,
/// but uniform escaping costs nothing and trusts nothing.
fn append_lines(out: &mut String, lines: &[SafeLine]) {
    for line in lines.iter().take(MAX_LINES_PER_KIND) {
        out.push_str("\n- (");
        out.push_str(&escape_html(&line.code));
        out.push_str(") ");
        out.push_str(&escape_html(&line.message));
    }
    let overflow = lines.len().saturating_sub(MAX_LINES_PER_KIND);
    if overflow > 0 {
        out.push_str("\n+");
        out.push_str(&overflow.to_string());
        out.push_str(" more");
    }
}

/// Escape the five characters Telegram's HTML parse mode treats specially.
fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Cut to at most `max_chars` characters. Working in `char`s can never split a grapheme the way
/// a byte cut would: multibyte letters survive whole or not at all.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use sqlx::types::Uuid;

    use super::{BODY_CAP_CHARS, render};
    use crate::projection::event::{OperationEvent, OperationStatus, SafeLine};

    /// One event with everything unset; tests override what they vary.
    fn an_event(status: OperationStatus) -> OperationEvent {
        OperationEvent {
            event_id: Uuid::now_v7(),
            occurred_at_secs: 1_786_960_800,
            correlation_id: "operation:018f0000-0000-7000-8000-000000000001".to_owned(),
            operation_id: Uuid::now_v7(),
            status,
            stage: None,
            progress_percent: None,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn status_branches_drive_display_not_stage_vocabulary() {
        for (status, lead) in [
            (OperationStatus::Accepted, "Accepted"),
            (OperationStatus::Queued, "Queued"),
            (OperationStatus::Running, "In progress"),
            (OperationStatus::Succeeded, "Completed"),
            (
                OperationStatus::PartiallySucceeded,
                "Completed with warnings",
            ),
            (OperationStatus::Failed, "Failed"),
            (OperationStatus::Cancelled, "Cancelled"),
        ] {
            let mut event = an_event(status);
            event.stage = Some("totally_unknown_producer_stage".to_owned());
            let html = render(&event);
            assert!(
                html.starts_with(&format!("<b>{lead}</b>")),
                "{status:?} must lead with its own display line, got {html}"
            );
        }
    }

    #[test]
    fn hostile_stage_error_and_warning_text_renders_escaped_and_truncated() {
        let mut event = an_event(OperationStatus::Running);
        event.stage = Some("<b>x</b> & <script>alert(1)</script>".to_owned());
        // 5000 chars mixing multibyte letters, so a byte-cut would split a grapheme.
        let hostile = "é日x".repeat(1700);
        assert_eq!(hostile.chars().count(), 5100);
        event.errors = vec![SafeLine {
            code: "content.source.broken".to_owned(),
            message: hostile,
        }];
        event.warnings = vec![
            SafeLine {
                code: "w.one".to_owned(),
                message: "first <warning>".to_owned(),
            },
            SafeLine {
                code: "w.two".to_owned(),
                message: "second & warning".to_owned(),
            },
            SafeLine {
                code: "w.three".to_owned(),
                message: "third \"warning\"".to_owned(),
            },
        ];

        let html = render(&event);

        assert!(
            !html.contains("<script>"),
            "raw markup from the stage must not survive"
        );
        assert!(
            html.contains("&lt;script&gt;"),
            "the stage is escaped, not dropped"
        );
        assert!(
            html.chars().count() <= BODY_CAP_CHARS,
            "the body respects the hard cap"
        );

        // The line-listing rules, on the same behavior family with short lines so nothing is
        // truncated away: three warnings listed escaped, the fourth folded into "+N more".
        let mut listed = an_event(OperationStatus::Running);
        listed.warnings = vec![
            SafeLine {
                code: "w.one".to_owned(),
                message: "first <warning>".to_owned(),
            },
            SafeLine {
                code: "w.two".to_owned(),
                message: "second & warning".to_owned(),
            },
            SafeLine {
                code: "w.three".to_owned(),
                message: "third \"warning\"".to_owned(),
            },
            SafeLine {
                code: "w.four".to_owned(),
                message: "fourth".to_owned(),
            },
        ];
        let listing = render(&listed);
        assert!(listing.contains("(w.one) first &lt;warning&gt;"));
        assert!(listing.contains("(w.two) second &amp; warning"));
        assert!(listing.contains("(w.three) third &quot;warning&quot;"));
        assert!(listing.contains("+1 more"));
        assert!(
            !listing.contains("fourth"),
            "beyond-the-cap lines are folded"
        );
    }

    #[test]
    fn render_is_deterministic_for_identical_events() {
        let mut event = an_event(OperationStatus::PartiallySucceeded);
        event.progress_percent = Some(75);
        event.stage = Some("normalizing".to_owned());
        assert_eq!(
            render(&event),
            render(&event),
            "identical bytes feed content_hash"
        );
    }
}
