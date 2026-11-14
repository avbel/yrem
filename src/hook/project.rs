//! The project a session works in, from its working directory.
//!
//! A project is the directory name of the repository's main worktree, so linked worktrees
//! (`fluidd-wt-306`, `.claude/worktrees/agent-x`) belong to the project they were cut from.

use std::fs;
use std::path::{Path, PathBuf};

/// Project name for `cwd`, or `None` outside a git repository. `aliases` maps the directory
/// name of a renamed repository to its current project name.
#[must_use]
pub fn resolve(cwd: &Path, aliases: &[(String, String)]) -> Option<String> {
    let (dir, git) = cwd
        .ancestors()
        .map(|d| (d, d.join(".git")))
        .find(|(_, g)| g.exists())?;
    let root = if git.is_dir() {
        dir.to_path_buf()
    } else {
        main_worktree(&git).unwrap_or_else(|| dir.to_path_buf())
    };
    let name = root.file_name()?.to_str()?;
    Some(
        aliases
            .iter()
            .find(|(from, _)| from == name)
            .map_or_else(|| name.to_owned(), |(_, to)| to.clone()),
    )
}

/// The main worktree behind a linked worktree's `.git` file (`gitdir: <main>/.git/worktrees/<n>`).
/// Submodules point elsewhere and have no main worktree.
fn main_worktree(git_file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(git_file).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let joined = git_file.parent()?.join(target);
    let gitdir = fs::canonicalize(&joined).unwrap_or(joined);
    let worktrees = gitdir.parent().filter(|p| p.ends_with("worktrees"))?;
    let common = worktrees.parent().filter(|p| p.ends_with(".git"))?;
    common.parent().map(Path::to_path_buf)
}
