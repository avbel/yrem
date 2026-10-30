//! Hand-written secret detectors over the text of one string leaf.
//!
//! Offsets are char offsets. Detectors run in the order private key, prefixed token,
//! URL credentials, credential field, entropy; a token claimed earlier is not reported again.

use std::ops::Range;

/// One match inside a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Hit {
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
}

struct Tok<'a> {
    raw: &'a str,
    text: &'a str,
    byte: usize,
    start: usize,
}

impl<'a> Tok<'a> {
    fn new(raw: &'a str, byte: usize, start: usize) -> Self {
        Self {
            raw,
            text: raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']),
            byte,
            start,
        }
    }

    fn end(&self) -> usize {
        self.start + self.text.chars().count()
    }
}

fn is_delim(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
}

fn tokenize(s: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for (chars, (byte, c)) in s.char_indices().enumerate() {
        if is_delim(c) {
            if let Some((b, ci)) = open.take() {
                out.push(Tok::new(&s[b..byte], b, ci));
            }
        } else if open.is_none() {
            open = Some((byte, chars));
        }
    }
    if let Some((b, ci)) = open {
        out.push(Tok::new(&s[b..], b, ci));
    }
    out
}

#[derive(Default)]
struct Claimed(Vec<Range<usize>>);

impl Claimed {
    fn overlaps(&self, r: &Range<usize>) -> bool {
        self.0.iter().any(|c| c.start < r.end && r.start < c.end)
    }

    fn claim(&mut self, r: Range<usize>) {
        self.0.push(r);
    }
}

pub(super) fn scan_text(s: &str, skip_entropy: bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut claimed = Claimed::default();
    private_keys(s, &mut hits, &mut claimed);
    let toks = tokenize(s);
    prefixed_tokens(&toks, &mut hits, &mut claimed);
    url_credentials(&toks, &mut hits, &mut claimed);
    credential_fields(s, &toks, &mut hits, &mut claimed);
    if !skip_entropy {
        high_entropy(&toks, &mut hits, &claimed);
    }
    hits
}

fn char_offset(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

fn private_keys(s: &str, hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    const BEGIN: &str = "-----BEGIN ";
    const DASHES: &str = "-----";
    let mut from = 0;
    while let Some(p) = s[from..].find(BEGIN) {
        let begin = from + p;
        let label_at = begin + BEGIN.len();
        let Some(close) = s[label_at..].find(DASHES) else {
            break;
        };
        let after_header = label_at + close + DASHES.len();
        if !s[label_at..label_at + close].contains("PRIVATE KEY") {
            from = after_header;
            continue;
        }
        let end = s[after_header..]
            .find("-----END ")
            .and_then(|e| {
                let tail = after_header + e + "-----END ".len();
                s[tail..].find(DASHES).map(|d| tail + d + DASHES.len())
            })
            .unwrap_or(s.len());
        let range = char_offset(s, begin)..char_offset(s, end);
        hits.push(Hit {
            kind: "private_key",
            start: range.start,
            end: range.end,
        });
        claimed.claim(range);
        from = end;
    }
}

fn alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

fn upper_digit(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

fn alnum_us(c: char) -> bool {
    alnum(c) || c == '_'
}

fn alnum_dash(c: char) -> bool {
    alnum(c) || c == '-'
}

fn base64url(c: char) -> bool {
    alnum_us(c) || c == '-'
}

struct Rule {
    kind: &'static str,
    prefixes: &'static [&'static str],
    charset: fn(char) -> bool,
    min: usize,
    max: usize,
}

macro_rules! rule {
    ($kind:literal, $prefixes:expr, $charset:expr, $min:expr, $max:expr $(,)?) => {
        Rule {
            kind: $kind,
            prefixes: $prefixes,
            charset: $charset,
            min: $min,
            max: $max,
        }
    };
}

const RULES: &[Rule] = &[
    rule!("aws_access_key", &["AKIA", "ASIA"], upper_digit, 16, 16),
    rule!(
        "github_token",
        &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        alnum,
        36,
        usize::MAX,
    ),
    rule!("github_token", &["github_pat_"], alnum_us, 22, usize::MAX),
    rule!("gitlab_token", &["glpat-"], base64url, 20, usize::MAX),
    rule!(
        "slack_token",
        &["xoxa-", "xoxb-", "xoxp-", "xoxr-", "xoxs-"],
        alnum_dash,
        10,
        usize::MAX,
    ),
    rule!("openai_anthropic_key", &["sk-"], base64url, 20, usize::MAX),
    rule!(
        "stripe_key",
        &["sk_live_", "sk_test_", "rk_live_", "rk_test_"],
        alnum,
        10,
        usize::MAX,
    ),
    rule!("stripe_key", &["whsec_"], alnum, 20, usize::MAX),
    rule!("google_api_key", &["AIza"], base64url, 35, 35),
    rule!("npm_token", &["npm_"], alnum, 36, usize::MAX),
    rule!("huggingface_token", &["hf_"], alnum, 30, usize::MAX),
    rule!(
        "age_secret_key",
        &["AGE-SECRET-KEY-1"],
        upper_digit,
        50,
        usize::MAX,
    ),
    rule!("ydb_token", &["ydb_"], alnum, 32, usize::MAX),
];

/// Kind, matched length in chars of a secret starting exactly at `cand`.
fn match_prefixed(cand: &str) -> Option<(&'static str, usize)> {
    for r in RULES {
        for p in r.prefixes {
            if let Some(rest) = cand.strip_prefix(p) {
                let run = rest.chars().take_while(|c| (r.charset)(*c)).count();
                if (r.min..=r.max).contains(&run) {
                    return Some((r.kind, p.len() + run));
                }
            }
        }
    }
    jwt_len(cand).map(|n| ("jwt", n))
}

fn jwt_len(cand: &str) -> Option<usize> {
    let mut parts = cand.split('.');
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    let shape = parts.next().is_none()
        && a.starts_with("eyJ")
        && b.starts_with("eyJ")
        && c.len() >= 16
        && [a, b, c].iter().all(|p| p.chars().all(base64url));
    shape.then(|| cand.chars().count())
}

fn prefixed_tokens(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for t in toks {
        if claimed.overlaps(&(t.start..t.end())) {
            continue;
        }
        let starts = std::iter::once(0).chain(t.text.match_indices(['=', ':']).map(|(i, _)| i + 1));
        for at in starts {
            let Some((kind, len)) = match_prefixed(&t.text[at..]) else {
                continue;
            };
            let start = t.start + char_offset(t.text, at);
            hits.push(Hit {
                kind,
                start,
                end: start + len,
            });
            claimed.claim(start..start + len);
            break;
        }
    }
}

fn url_credentials(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for t in toks {
        let range = t.start..t.end();
        if claimed.overlaps(&range) || !has_userinfo_password(t.text) {
            continue;
        }
        hits.push(Hit {
            kind: "url_credentials",
            start: range.start,
            end: range.end,
        });
        claimed.claim(range);
    }
}

fn has_userinfo_password(token: &str) -> bool {
    let Some((_, rest)) = token.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    authority
        .rsplit_once('@')
        .and_then(|(userinfo, _)| userinfo.split_once(':'))
        .is_some_and(|(_, password)| !password.is_empty())
}

const KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "client_secret",
    "api_key",
    "apikey",
    "api-key",
    "secret_key",
    "private_key",
    "access_key",
    "aws_secret_access_key",
    "access_token",
    "auth_token",
    "refresh_token",
    "token",
    "authorization",
];

pub(super) fn is_credential_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    KEYWORDS.iter().any(|k| {
        lower.strip_suffix(k).is_some_and(|head| {
            head.chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        })
    })
}

fn is_placeholder(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    lower.starts_with(['<', '$'])
        || lower.starts_with("{{")
        || lower.starts_with("***")
        || matches!(
            lower.as_str(),
            "redacted" | "changeme" | "placeholder" | "null" | "none" | "hidden" | "example"
        )
        || lower.chars().all(|c| c == 'x')
        || lower.chars().all(|c| c == '*')
}

/// Whether `v` has the shape of a credential value (not a placeholder).
pub(super) fn is_credential_value(v: &str) -> bool {
    v.chars().count() >= 8
        && !v.chars().any(char::is_whitespace)
        && (v.chars().any(|c| c.is_ascii_digit())
            || (v.chars().any(char::is_uppercase) && v.chars().any(char::is_lowercase))
            || v.chars().count() >= 20)
        && !is_placeholder(v)
}

/// Token index and byte offset where the value of a keyword at token `i` starts.
fn value_slot(toks: &[Tok], i: usize) -> Option<(usize, usize)> {
    let raw = toks[i].raw;
    let sep = |s: &str| s.starts_with(['=', ':']);
    match raw.find(['=', ':']) {
        Some(p) if is_credential_name(&raw[..p]) => Some(if raw.len() > p + 1 {
            (i, p + 1)
        } else {
            (i + 1, 0)
        }),
        None if raw.eq_ignore_ascii_case("bearer") => Some((i + 1, 0)),
        None if is_credential_name(raw) => {
            let next = toks.get(i + 1)?.raw;
            if !sep(next) {
                return None;
            }
            Some(if next.len() > 1 {
                (i + 1, 1)
            } else {
                (i + 2, 0)
            })
        }
        Some(_) | None => None,
    }
}

fn credential_fields(s: &str, toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for i in 0..toks.len() {
        let Some((mut at, mut off)) = value_slot(toks, i) else {
            continue;
        };
        let bearer_value = |t: &Tok, off: usize| {
            t.raw[off..]
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')'])
                .eq_ignore_ascii_case("bearer")
        };
        if toks.get(at).is_some_and(|t| bearer_value(t, off)) {
            (at, off) = (at + 1, 0);
        }
        let Some(t) = toks.get(at) else {
            continue;
        };
        let value = t.raw[off..].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
        let head = s[..t.byte + off].trim_end();
        let start = t.start + char_offset(t.raw, off);
        let range = start..start + value.chars().count();
        if head.ends_with('<') || head.ends_with("{{") || claimed.overlaps(&range) {
            continue;
        }
        if !is_credential_value(value) {
            continue;
        }
        hits.push(Hit {
            kind: "credential_field",
            start: range.start,
            end: range.end,
        });
        claimed.claim(range);
    }
}

fn entropy(s: &str) -> f64 {
    let mut counts = [0_u32; 128];
    for b in s.bytes() {
        counts[usize::from(b) & 127] += 1;
    }
    let total = f64::from(counts.iter().sum::<u32>());
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / total;
            -p * p.log2()
        })
        .sum()
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn looks_high_entropy(t: &str) -> bool {
    t.len() >= 32
        && t.chars()
            .all(|c| alnum(c) || matches!(c, '+' | '/' | '=' | '_' | '-'))
        && t.chars().filter(char::is_ascii_digit).count() >= 3
        && t.chars().any(|c| c.is_ascii_uppercase())
        && t.chars().any(|c| c.is_ascii_lowercase())
        && !is_uuid(t)
        && entropy(t) >= 4.5
}

fn high_entropy(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &Claimed) {
    for t in toks {
        let range = t.start..t.end();
        if looks_high_entropy(t.text) && !claimed.overlaps(&range) {
            hits.push(Hit {
                kind: "high_entropy",
                start: range.start,
                end: range.end,
            });
        }
    }
}
