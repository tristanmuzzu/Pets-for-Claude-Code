//! Turning whatever a human or a model wrote into one line a card can show.
//!
//! Shared by the hook (which sees prompts and final answers) and the narrator
//! (which sees the assistant talking mid-turn), because both are answering the
//! same question: of everything in this text, which single line is worth the
//! one line a card has?

/// The first line of a prompt or an answer that is worth putting on a card.
///
/// Two things kept turning up in that slot and telling nobody anything: an
/// opening line that is a greeting rather than a sentence ("Tristan,"), and a
/// prompt that is a dragged-in file, which arrives as an absolute path and
/// takes the whole line to say one filename. `None` means the text has nothing
/// to say, which is different from having nothing in it.
pub fn summary_of(text: &str) -> Option<String> {
    summary_within(text, 90)
}

pub fn summary_within(text: &str, limit: usize) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(strip_markup)
        .map(|line| shorten_paths(&line))
        .find(|line| !is_throat_clearing(line))
        .map(|line| truncate(&line, limit))
}

/// A line that only opens the conversation. Kept deliberately narrow: a short
/// sentence is usually the most informative thing in a prompt, so only a
/// trailing comma or colon (which promises the real line is the next one) or a
/// bare acknowledgement counts.
pub fn is_throat_clearing(line: &str) -> bool {
    let words = line.split_whitespace().count();
    if words <= 3 && line.ends_with([',', ':']) {
        return true;
    }
    let bare = line.trim_end_matches(['.', '!', ',', ':']).to_lowercase();
    matches!(
        bare.as_str(),
        "ok" | "okay" | "hi" | "hey" | "hello" | "thanks" | "thank you" | "sure" | "yes" | "no"
    ) || line.chars().all(|c| !c.is_alphanumeric())
}

/// Drops the markdown that only makes sense in a document.
///
/// A heading's `##` and a bullet's `-` are structure, not words, and the card
/// has no structure to give them. Emphasis markers are left where they fall
/// inside a sentence: `**not** this` reads fine, and hunting pairs across a
/// truncated line is how you end up with a stray asterisk.
pub fn strip_markup(line: &str) -> String {
    let trimmed = line.trim();
    let without_bullet = trimmed
        .trim_start_matches(['#', '>'])
        .trim_start()
        .to_string();
    let without_bullet = match without_bullet.strip_prefix("- ") {
        Some(rest) => rest.to_string(),
        None => without_bullet,
    };
    // A line that is nothing but emphasis is a heading in disguise.
    without_bullet
        .trim_matches(|c| c == '*' || c == '_' || c == '`')
        .trim()
        .to_string()
}

/// Replaces file paths with the file's name.
///
/// A dragged-in or `@`-mentioned file arrives as `@"C:\Users\me\Downloads\CLAUDE
/// 1.md"`, and a card that is one line tall should spend it on "CLAUDE 1.md".
/// URLs are left alone: the host and the path are the whole point of one.
pub fn shorten_paths(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        // Only ever look at whole words. Testing every offset would find a
        // "path" inside `https://example.com/a/b` and throw the host away.
        let spaces = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..spaces]);
        rest = &rest[spaces..];
        if rest.is_empty() {
            break;
        }

        let marked = rest.starts_with('@');
        let body = if marked { &rest[1..] } else { rest };
        let (token, consumed) = match body.strip_prefix('"') {
            Some(quoted) => match quoted.find('"') {
                // A quoted path: the quotes are how it survives its spaces.
                Some(end) => (&quoted[..end], end + 2),
                None => (quoted, quoted.len() + 1),
            },
            None => {
                let end = body.find(char::is_whitespace).unwrap_or(body.len());
                (&body[..end], end)
            }
        };
        let width = consumed + usize::from(marked);

        if looks_like_path(token, marked) {
            out.push_str(&basename(token));
        } else {
            out.push_str(&rest[..width]);
        }
        rest = &rest[width..];
    }
    out.trim().to_string()
}

/// `marked` says the token was written as an `@` mention, which is Claude
/// Code's own way of saying "this is a file" and settles the question.
pub fn looks_like_path(token: &str, marked: bool) -> bool {
    if token.len() < 3 || token.contains("://") {
        return false;
    }
    if !token.contains('\\') && !token.contains('/') {
        return false;
    }
    let rooted = token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("~/")
        || token.as_bytes().get(1).map(|c| *c == b':').unwrap_or(false);
    // Unmarked, a single separator is as likely to be "either/or" as a path.
    marked || rooted || token.matches(['\\', '/']).count() > 1
}

pub fn basename(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

pub fn basename_or(path: &str, fallback: &str) -> String {
    if path.is_empty() {
        fallback.to_string()
    } else {
        basename(path)
    }
}

pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_greeting_is_skipped_for_the_line_after_it() {
        assert_eq!(
            summary_of("Tristan,\n\nFixed the timezone bug.").as_deref(),
            Some("Fixed the timezone bug.")
        );
        assert_eq!(summary_of("Tristan,"), None);
    }

    #[test]
    fn a_dropped_file_is_named_not_pathed() {
        assert_eq!(
            summary_of("@\"C:\\Users\\acer\\Downloads\\CLAUDE 1.md\"").as_deref(),
            Some("CLAUDE 1.md")
        );
    }

    #[test]
    fn urls_survive_intact() {
        assert_eq!(
            shorten_paths("read https://example.com/a/b/c.html"),
            "read https://example.com/a/b/c.html"
        );
        assert_eq!(shorten_paths("either/or"), "either/or");
    }

    #[test]
    fn markdown_structure_is_not_words() {
        assert_eq!(strip_markup("## Now the frontend"), "Now the frontend");
        assert_eq!(strip_markup("- reads the file"), "reads the file");
        assert_eq!(strip_markup("**Done.**"), "Done.");
        assert_eq!(strip_markup("keep **this** one"), "keep **this** one");
    }

    #[test]
    fn truncation_never_splits_a_character() {
        let truncated = truncate("ααααα", 3);
        assert_eq!(truncated.chars().count(), 3);
        assert!(truncated.ends_with('…'));
    }
}
