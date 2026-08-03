//! Spotting the commands you would not want to approve without looking.
//!
//! This exists to answer one question on a card that says "needs you": is this
//! a routine approval, or the kind you should pause the video for. It is a
//! hint and nothing else — it never blocks, allows, or denies anything, and a
//! wrong answer costs at most a missing or spurious line of text.
//!
//! Which is why it errs towards silence. The command string comes from a model
//! and a false badge on a quiet overlay is noise you learn to ignore, while a
//! missed one just leaves the card as informative as it was before.

/// Long enough for any real command, short enough that a pathological input
/// cannot make the scan expensive.
const MAX_SCAN: usize = 4096;
const MAX_SEGMENTS: usize = 50;

/// Returns a short reason when the command looks irreversible.
pub fn irreversible(command: &str) -> Option<&'static str> {
    let scanned: String = command.chars().take(MAX_SCAN).collect();
    for segment in segments(&scanned).into_iter().take(MAX_SEGMENTS) {
        if let Some(reason) = classify_segment(&segment) {
            return Some(reason);
        }
    }
    None
}

/// Splits a command line on shell separators that are *not* inside quotes.
///
/// The quoting matters more than it looks: `git commit -m "docs && git push
/// --force"` contains the text of a force push inside a message, and a naive
/// split would flag every commit whose message mentions one.
fn segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' if double => {
                // Escaped character inside double quotes: consume both so an
                // escaped quote does not flip the state.
                current.push(c);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' if !double => {
                single = !single;
                current.push(c);
            }
            '"' if !single => {
                double = !double;
                current.push(c);
            }
            '\n' | ';' if !single && !double => {
                out.push(std::mem::take(&mut current));
            }
            '&' | '|' if !single && !double => {
                // One or two: `&&`, `||` and a plain pipe all start a new
                // command position.
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

/// Drops the words that can sit in front of a command without being one.
fn command_words(segment: &str) -> Vec<String> {
    let mut words: Vec<String> = segment
        .split_whitespace()
        .map(|w| w.trim_matches(['"', '\'']).to_string())
        .filter(|w| !w.is_empty())
        .collect();
    while let Some(first) = words.first() {
        let lowered = first.to_lowercase();
        let wrapper = matches!(
            lowered.as_str(),
            "sudo" | "doas" | "env" | "nohup" | "time" | "command" | "exec" | "then" | "do"
        );
        // `FOO=bar cmd …`
        let assignment = first
            .split_once('=')
            .map(|(name, _)| !name.is_empty() && !name.contains('/'))
            .unwrap_or(false);
        if wrapper || assignment {
            words.remove(0);
        } else {
            break;
        }
    }
    words
}

fn classify_segment(segment: &str) -> Option<&'static str> {
    let words = command_words(segment);
    let head = words.first()?.to_lowercase();
    // Compare against the program name, not the path it was invoked through.
    let program = head
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&head)
        .trim_end_matches(".exe")
        .to_string();
    let rest: Vec<String> = words[1..].iter().map(|w| w.to_lowercase()).collect();
    let has = |flag: &str| rest.iter().any(|w| w == flag);
    let starts = |sub: &[&str]| rest.iter().zip(sub).all(|(w, s)| w == s) && rest.len() >= sub.len();

    match program.as_str() {
        "rm" => {
            let recursive = rest.iter().any(|w| is_short_flag(w, 'r') || w == "--recursive");
            let forced = rest.iter().any(|w| is_short_flag(w, 'f') || w == "--force");
            (recursive && forced).then_some("Deletes a directory tree")
        }
        "rmdir" if has("/s") && has("/q") => Some("Deletes a directory tree"),
        "rd" if has("/s") => Some("Deletes a directory tree"),
        "del" if has("/s") || has("/q") => Some("Deletes files without confirming"),
        "remove-item" => {
            let recursive = rest.iter().any(|w| w.starts_with("-recurse"));
            let forced = rest.iter().any(|w| w.starts_with("-force"));
            (recursive && forced).then_some("Deletes a directory tree")
        }
        "git" => git(&rest),
        "npm" | "pnpm" | "yarn" | "bun" if starts(&["publish"]) => Some("Publishes a package"),
        "cargo" if starts(&["publish"]) => Some("Publishes a package"),
        "twine" if starts(&["upload"]) => Some("Publishes a package"),
        "gh" if starts(&["repo", "delete"]) => Some("Deletes a repository"),
        "terraform" | "tofu" if starts(&["destroy"]) => Some("Tears down infrastructure"),
        "kubectl" if starts(&["delete"]) => Some("Deletes cluster resources"),
        "helm" if starts(&["uninstall"]) || starts(&["delete"]) => Some("Removes a release"),
        "docker" if starts(&["system", "prune"]) => Some("Prunes Docker state"),
        "dd" => Some("Writes raw blocks to a device"),
        "mkfs" => Some("Formats a filesystem"),
        "format-volume" => Some("Formats a volume"),
        "shutdown" | "reboot" => Some("Restarts the machine"),
        "psql" | "mysql" | "sqlite3" | "mongosh" | "mongo" => {
            let sql = segment.to_lowercase();
            let destructive = ["drop table", "drop database", "drop schema", "truncate "]
                .iter()
                .any(|needle| sql.contains(needle));
            destructive.then_some("Destroys database contents")
        }
        _ => None,
    }
}

fn git(rest: &[String]) -> Option<&'static str> {
    let sub = rest.first()?.as_str();
    let args = &rest[1..];
    let has = |flag: &str| args.iter().any(|w| w == flag);
    match sub {
        "push" => {
            // `--force-with-lease` is the careful version and refuses to
            // clobber work it has not seen. Flagging it would train you to
            // ignore the badge.
            if has("--force") || args.iter().any(|w| is_short_flag(w, 'f')) {
                Some("Force-pushes over remote history")
            } else if has("--delete") || has("-d") || args.iter().any(|w| w.starts_with(':')) {
                Some("Deletes a remote branch")
            } else {
                None
            }
        }
        "reset" if has("--hard") => Some("Discards uncommitted work"),
        "clean" => (has("-f") || has("-fd") || has("-fdx") || has("--force"))
            .then_some("Deletes untracked files"),
        "branch" if has("-d") => Some("Force-deletes a branch"),
        "filter-branch" | "filter-repo" => Some("Rewrites history"),
        _ => None,
    }
}

/// True for a clustered short flag containing `letter`, e.g. `-rf` for `r`.
fn is_short_flag(word: &str, letter: char) -> bool {
    word.starts_with('-')
        && !word.starts_with("--")
        && word.chars().skip(1).any(|c| c == letter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_the_obvious_ones() {
        for command in [
            "rm -rf build",
            "sudo rm -r -f /var/tmp/cache",
            "git push --force origin main",
            "git reset --hard HEAD~3",
            "npm publish",
            "terraform destroy -auto-approve",
            "kubectl delete deployment api",
            "gh repo delete acme/thing",
        ] {
            assert!(
                irreversible(command).is_some(),
                "{command} should be flagged"
            );
        }
    }

    #[test]
    fn leaves_ordinary_work_alone() {
        for command in [
            "npm test",
            "git push origin main",
            "git status",
            "rm build/tmp.txt",
            "cargo build --release",
            "kubectl get pods",
            "docker compose up -d",
        ] {
            assert!(
                irreversible(command).is_none(),
                "{command} should not be flagged"
            );
        }
    }

    /// The reason the splitter is quote-aware. Without it every commit whose
    /// message mentions a force push would raise the badge.
    #[test]
    fn text_inside_quotes_is_not_a_command() {
        assert!(irreversible(r#"git commit -m "docs && git push --force""#).is_none());
        assert!(irreversible(r#"echo 'rm -rf /'"#).is_none());
    }

    #[test]
    fn a_later_segment_still_counts() {
        assert!(irreversible("npm run build && rm -rf dist").is_some());
        assert!(irreversible("cd repo; git reset --hard").is_some());
    }

    /// The careful version of a force push refuses to clobber work it has not
    /// seen, so flagging it would only teach you to ignore the badge.
    #[test]
    fn force_with_lease_is_not_flagged() {
        assert!(irreversible("git push --force-with-lease").is_none());
    }

    #[test]
    fn a_path_prefix_does_not_hide_the_command() {
        assert!(irreversible("/usr/bin/rm -rf /tmp/x").is_some());
        assert!(irreversible("C:\\Windows\\System32\\shutdown.exe /r").is_some());
    }

    #[test]
    fn absurd_input_is_bounded_and_quiet() {
        let long = "a".repeat(50_000);
        assert!(irreversible(&long).is_none());
        let many = "true; ".repeat(500) + "rm -rf x";
        // Past the segment cap the scan stops rather than growing without
        // bound; missing a hint is the acceptable failure here.
        let _ = irreversible(&many);
    }
}
