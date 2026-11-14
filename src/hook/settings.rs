//! `YREM_HOOK_*` settings.

use super::gate::Gate;

/// How often the briefing carries maintenance lines (open conflicts, known gaps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Maintenance {
    Off,
    /// Every briefing.
    Always,
    /// At most once per 24 hours across all sessions.
    #[default]
    Daily,
}

impl Maintenance {
    fn parse(v: &str) -> Option<Self> {
        match v.to_ascii_lowercase().as_str() {
            "0" | "false" | "no" | "off" => Some(Self::Off),
            "1" | "true" | "yes" | "on" => Some(Self::Always),
            "daily" => Some(Self::Daily),
            _ => None,
        }
    }
}

/// `YREM_HOOK_*` settings.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools, reason = "independent on/off switches")]
pub struct Settings {
    /// An explicit namespace: recall is filtered to it and it is the project.
    pub namespace: Option<String>,
    /// Directory name to project name, for renamed repositories.
    pub aliases: Vec<(String, String)>,
    pub digest: bool,
    pub decisions: usize,
    pub maintenance: Maintenance,
    pub recall: bool,
    pub top_k: usize,
    pub gate: Gate,
    pub capture: bool,
    pub think: bool,
    pub track_session: bool,
}

impl Settings {
    #[must_use]
    pub fn from_env(env: &impl Fn(&str) -> Option<String>) -> Self {
        let get = |k: &str| {
            env(&format!("YREM_HOOK_{k}"))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let flag = |k: &str, default: bool| {
            get(k).map_or(default, |v| {
                !matches!(
                    v.to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
        };
        let num = |k: &str, default: usize| get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
        let float = |k: &str, default: f64| get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
        Self {
            namespace: get("NAMESPACE"),
            aliases: get("NAMESPACE_ALIASES").map_or_else(Vec::new, |v| parse_aliases(&v)),
            digest: flag("DIGEST", true),
            decisions: num("DECISIONS", 5),
            maintenance: get("MAINTENANCE").map_or_else(Maintenance::default, |v| {
                Maintenance::parse(&v).unwrap_or_else(|| {
                    eprintln!(
                        "yrem hook: YREM_HOOK_MAINTENANCE must be 0, 1 or daily; using daily"
                    );
                    Maintenance::Daily
                })
            }),
            recall: flag("RECALL", true),
            top_k: num("TOP_K", 3),
            gate: Gate {
                min_similarity: float("MIN_SIMILARITY", 0.50),
                min_similarity_other: float("MIN_SIMILARITY_OTHER", 0.60),
                include_captured: flag("INJECT_CAPTURED", false),
                excluded_namespaces: get("EXCLUDE_NAMESPACES")
                    .map(|v| {
                        v.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                project: None,
            },
            capture: flag("CAPTURE", false),
            think: flag("THINK", true),
            track_session: flag("TRACK_SESSION", true),
        }
    }
}

/// `old=new,old2=new2`; entries without both sides are ignored.
fn parse_aliases(v: &str) -> Vec<(String, String)> {
    v.split(',')
        .filter_map(|pair| {
            let (from, to) = pair.split_once('=')?;
            let (from, to) = (from.trim(), to.trim());
            (!from.is_empty() && !to.is_empty()).then(|| (from.to_owned(), to.to_owned()))
        })
        .collect()
}
