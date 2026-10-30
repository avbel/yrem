//! Configuration: CLI flags > environment > config file.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use serde::Deserialize;

/// Contents of `config.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    url: Option<String>,
    token: Option<String>,
    token_cmd: Option<String>,
}

/// Values given on the command line (clap already folds in `YREM_*`).
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub url: Option<String>,
    pub token: Option<String>,
    pub config: Option<PathBuf>,
}

/// Where a resolved value came from, for `doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Flag,
    Env,
    File,
    TokenCmd,
    Unset,
}

/// Resolved configuration.
#[derive(Clone)]
pub struct Config {
    pub url: String,
    pub token: Option<String>,
    pub url_origin: Origin,
    pub token_origin: Origin,
    pub file: Option<PathBuf>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("url_origin", &self.url_origin)
            .field("token_origin", &self.token_origin)
            .field("file", &self.file)
            .finish()
    }
}

impl Config {
    /// Resolves the configuration from the process environment.
    ///
    /// # Errors
    /// Unreadable or invalid config file, failing `token_cmd`, or no server URL.
    pub fn load(overrides: &Overrides) -> anyhow::Result<Self> {
        Self::resolve(overrides, |k| std::env::var(k).ok())
    }

    /// Resolves the configuration with an injectable environment.
    ///
    /// # Errors
    /// See [`Config::load`].
    pub fn resolve(
        overrides: &Overrides,
        env: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        let non_empty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        let (path, explicit) = match &overrides.config {
            Some(p) => (Some(p.clone()), true),
            None => (default_path(&env), false),
        };
        let file = match &path {
            Some(p) => read_file(p, explicit)?,
            None => None,
        };
        let loaded = file.is_some();
        let file = file.unwrap_or_default();

        let (url, url_origin) = if let Some(u) = non_empty(overrides.url.clone()) {
            (u, Origin::Flag)
        } else if let Some(u) = non_empty(env("YANTRIKDB_SERVER_URL")) {
            (u, Origin::Env)
        } else if let Some(u) = non_empty(file.url) {
            (u, Origin::File)
        } else {
            bail!("no server URL: pass --url, set YREM_URL, or add `url` to the config file");
        };

        let (token, token_origin) = if let Some(t) = non_empty(overrides.token.clone()) {
            (Some(t), Origin::Flag)
        } else if let Some(t) = non_empty(env("YANTRIKDB_TOKEN")) {
            (Some(t), Origin::Env)
        } else if let Some(t) = non_empty(file.token) {
            (Some(t), Origin::File)
        } else if let Some(cmd) = non_empty(file.token_cmd) {
            (Some(run_token_cmd(&cmd)?), Origin::TokenCmd)
        } else {
            (None, Origin::Unset)
        };

        Ok(Self {
            url: url.trim_end_matches('/').to_owned(),
            token,
            url_origin,
            token_origin,
            file: path.filter(|_| loaded),
        })
    }
}

fn default_path(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = match env("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        Some(x) => PathBuf::from(x),
        None => env("HOME")
            .map(PathBuf::from)
            .or_else(std::env::home_dir)?
            .join(".config"),
    };
    Some(base.join("yrem").join("config.toml"))
}

fn read_file(path: &Path, explicit: bool) -> anyhow::Result<Option<FileConfig>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let cfg = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(cfg))
}

fn run_token_cmd(cmd: &str) -> anyhow::Result<String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .context("running token_cmd")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy_owned(out.stderr);
        bail!("token_cmd failed ({}): {}", out.status, stderr.trim());
    }
    let stdout = String::from_utf8_lossy_owned(out.stdout);
    if stdout.contains(char::REPLACEMENT_CHARACTER) {
        bail!("token_cmd output is not valid UTF-8");
    }
    let token = stdout.trim();
    if token.is_empty() {
        bail!("token_cmd printed nothing");
    }
    Ok(token.to_owned())
}
