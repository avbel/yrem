//! The project a session works in, from its working directory.
//!
//! A project is the directory name of the repository's main worktree, so linked worktrees
//! (`fluidd-wt-306`, `.claude/worktrees/agent-x`) belong to the project they were cut from.
//! The home directory itself is the project `home`: it is where agents start when no
//! repository is meant, and without a name its rows would land in the server's `default`.

use std::fs;
use std::path::{Path, PathBuf};

/// The default project of the home directory.
pub const HOME_NAMESPACE: &str = "home";

/// What turns a directory into a project name.
#[derive(Debug, Clone, Copy)]
pub struct Names<'a> {
    /// Directory name of a renamed repository to its current project name.
    pub aliases: &'a [(String, String)],
    pub home: Option<&'a Path>,
    pub home_namespace: &'a str,
}

/// Project name for `cwd`: the home namespace for the home directory itself, else the
/// repository's name, or `None` outside a git repository.
#[must_use]
pub fn resolve_named(cwd: &Path, names: &Names) -> Option<String> {
    if names.home.is_some_and(|home| same_dir(cwd, home)) {
        return Some(names.home_namespace.to_owned());
    }
    resolve(cwd, names.aliases)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || real(a) == real(b)
}

/// Repository name for `cwd`, or `None` outside a git repository. `aliases` maps the directory
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
