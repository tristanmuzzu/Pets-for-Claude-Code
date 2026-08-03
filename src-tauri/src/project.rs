//! Works out which project a session belongs to.
//!
//! The folder a session is running in is often not the project: agents work in
//! git worktrees, and scripted runs work in temp directories with generated
//! names. Titling a card after `basename(cwd)` produces things like
//! `design-quality__nocaveman__r3`, which means nothing to the person reading
//! it. Resolve to the repository instead.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Display name — the basename of the root.
    pub name: String,
    pub root: PathBuf,
    /// Set when the session is not sitting at the project root.
    pub workspace: String,
    /// Temp-rooted, i.e. a throwaway session from a script.
    pub scratch: bool,
}

pub fn resolve(cwd: &str) -> Project {
    let cwd_path = PathBuf::from(cwd);
    // A git repository beats everything: it resolves worktrees back to the
    // repository they belong to, which is what a person means by "project".
    let root = git_root(&cwd_path)
        // Claude Code exports this to every command hook. It points at the
        // worktree rather than the parent repo, so it is only a fallback.
        .or_else(|| std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from))
        .unwrap_or_else(|| cwd_path.clone());

    let name = basename(&root);
    let workspace = if root == cwd_path {
        String::new()
    } else {
        basename(&cwd_path)
    };

    Project {
        scratch: is_scratch(&root),
        name,
        root,
        workspace,
    }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Walks up looking for `.git`, following the worktree pointer file back to the
/// repository that owns it. Pure filesystem — a hook fires often enough that
/// shelling out to git on every event would be wasteful.
fn git_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let marker = dir.join(".git");
        if marker.is_dir() {
            return Some(dir.to_path_buf());
        }
        if marker.is_file() {
            // "gitdir: C:/repo/.git/worktrees/<name>"
            let text = std::fs::read_to_string(&marker).ok()?;
            let raw = text.split_once("gitdir:")?.1.trim().replace('\\', "/");
            let idx = raw.find("/.git/worktrees/")?;
            return Some(PathBuf::from(&raw[..idx]));
        }
    }
    None
}

fn is_scratch(root: &Path) -> bool {
    let lower = root.to_string_lossy().to_lowercase().replace('\\', "/");
    lower.contains("/appdata/local/temp/")
        || lower.contains("/temp/")
        || lower.starts_with("/tmp/")
        || lower.contains("/var/folders/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_directory_is_its_own_project() {
        let project = resolve("C:\\Users\\me\\Desktop\\prokitchens-project");
        assert_eq!(project.name, "prokitchens-project");
        assert!(project.workspace.is_empty());
        assert!(!project.scratch);
    }

    #[test]
    fn temp_rooted_sessions_are_marked_scratch() {
        assert!(is_scratch(Path::new(
            "C:\\Users\\me\\AppData\\Local\\Temp\\harness-ablation\\runs\\design-quality__r3"
        )));
        assert!(is_scratch(Path::new("/tmp/scratch/run-4")));
        assert!(!is_scratch(Path::new("C:\\Users\\me\\Documents\\real-project")));
    }

    #[test]
    fn a_worktree_resolves_to_the_repository_that_owns_it() {
        let dir = std::env::temp_dir().join(format!("pipsqueak-wt-{}", std::process::id()));
        let repo = dir.join("my-repo");
        let tree = dir.join("feature-x");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("feature-x")).unwrap();
        std::fs::write(
            tree.join(".git"),
            format!(
                "gitdir: {}/.git/worktrees/feature-x\n",
                repo.to_string_lossy()
            ),
        )
        .unwrap();

        let found = git_root(&tree).expect("worktree should resolve");
        assert_eq!(found, repo);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
