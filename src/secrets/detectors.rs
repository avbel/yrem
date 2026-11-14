//! Hand-written secret detectors over the text of one string leaf.
//!
//! Offsets are char offsets. Detectors run in the order private key, prefixed token,
//! URL credentials, credential field, PIN code, entropy; a token claimed earlier is not
//! reported again.

use std::ops::Range;

mod pin;

pub(super) use pin::{is_name as is_pin_name, is_value as is_pin_value};

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

const TRAILING: [char; 7] = ['.', ',', ';', ':', '!', '?', ')'];

/// Wrapper characters (markdown, table pipes, zero-width marks) tolerated before a token body.
const MAX_WRAPPER: usize = 8;

impl<'a> Tok<'a> {
    fn new(raw: &'a str, byte: usize, start: usize) -> Self {
        Self {
            raw,
            text: raw.trim_end_matches(TRAILING),
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
            '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
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
    let blocks = armored_blocks(s);
    private_keys(&blocks, &mut hits, &mut claimed);
    let toks = tokenize(s);
    prefixed_tokens(&toks, &mut hits, &mut claimed);
    url_credentials(&toks, &mut hits, &mut claimed);
    credential_fields(s, &toks, &mut hits, &mut claimed);
    pin::scan(&toks, &mut hits, &mut claimed);
    if !skip_entropy {
        public_material(&blocks, &toks, &mut claimed);
        high_entropy(&toks, &mut hits, &claimed);
    }
    hits
}

fn char_offset(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Armor {
    Private,
    Public,
}

/// PEM blocks as char ranges: private keys, and certificates / public keys (which are not secret).
fn armored_blocks(s: &str) -> Vec<(Armor, Range<usize>)> {
    const BEGIN: &str = "-----BEGIN ";
    const DASHES: &str = "-----";
    let mut blocks = Vec::new();
    let mut from = 0;
    let mut cursor = (0, 0);
    let mut chars_at = |byte: usize| {
        cursor = (byte, cursor.1 + s[cursor.0..byte].chars().count());
        cursor.1
    };
    while let Some(p) = s[from..].find(BEGIN) {
        let begin = from + p;
        let label_at = begin + BEGIN.len();
        let Some(close) = s[label_at..].find(DASHES) else {
            break;
        };
        let after_header = label_at + close + DASHES.len();
        let label = &s[label_at..label_at + close];
        let armor = if label.contains("PRIVATE KEY") {
            Armor::Private
        } else if label.contains("PUBLIC KEY") || label.contains("CERTIFICATE") {
            Armor::Public
        } else {
            from = after_header;
            continue;
        };
        let end = s[after_header..]
            .find("-----END ")
            .and_then(|e| {
                let tail = after_header + e + "-----END ".len();
                s[tail..].find(DASHES).map(|d| tail + d + DASHES.len())
            })
            .unwrap_or(s.len());
        let start = chars_at(begin);
        blocks.push((armor, start..chars_at(end)));
        from = end;
    }
    blocks
}

fn private_keys(blocks: &[(Armor, Range<usize>)], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for (_, range) in blocks.iter().filter(|(a, _)| *a == Armor::Private) {
        hits.push(Hit {
            kind: "private_key",
            start: range.start,
            end: range.end,
        });
        claimed.claim(range.clone());
    }
}

/// Claims data that looks random but is public, so the entropy detector skips it.
fn public_material(blocks: &[(Armor, Range<usize>)], toks: &[Tok], claimed: &mut Claimed) {
    for (_, range) in blocks.iter().filter(|(a, _)| *a == Armor::Public) {
        claimed.claim(range.clone());
    }
    for pair in toks.windows(2) {
        let kind = pair[0].text;
        if matches!(kind, "ssh-rsa" | "ssh-ed25519" | "ssh-dss") || kind.starts_with("ecdsa-sha2-")
        {
            claimed.claim(pair[1].start..pair[1].end());
        }
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
    /// Run must hold a digit or mixed case, so slugs like `sk-payment-gateway-production` pass.
    shaped: bool,
}

macro_rules! rule {
    ($kind:literal, $prefixes:expr, $charset:expr, $min:expr, $max:expr $(,)?) => {
        Rule {
            kind: $kind,
            prefixes: $prefixes,
            charset: $charset,
            min: $min,
            max: $max,
            shaped: false,
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
    Rule {
        shaped: true,
        ..rule!("openai_anthropic_key", &["sk-"], base64url, 20, usize::MAX)
    },
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
            let Some(rest) = cand.strip_prefix(p) else {
                continue;
            };
            let run = rest.find(|c| !(r.charset)(c)).unwrap_or(rest.len());
            let body = &rest[..run];
            if (r.min..=r.max).contains(&run)
                && !is_placeholder(body)
                && (!r.shaped || digit_or_mixed(body))
            {
                return Some((r.kind, p.len() + run));
            }
        }
    }
    jwt_len(cand).map(|n| ("jwt", n))
}

/// Only the leading JWT-charset run is measured, so `?id_token=<jwt>&state=x` matches and a
/// long non-JWT string costs O(run), not O(rest of the token).
fn jwt_len(cand: &str) -> Option<usize> {
    let run = cand
        .find(|c: char| !(base64url(c) || c == '.'))
        .unwrap_or(cand.len());
    let mut parts = cand[..run].split('.');
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    let shape =
        parts.next().is_none() && a.starts_with("eyJ") && b.starts_with("eyJ") && c.len() >= 16;
    shape.then_some(run)
}

fn wrapper_len(s: &str) -> usize {
    s.char_indices()
        .take(MAX_WRAPPER)
        .find(|(_, c)| alnum(*c))
        .map_or(0, |(i, _)| i)
}

fn prefixed_tokens(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for t in toks {
        if claimed.overlaps(&(t.start..t.end())) {
            continue;
        }
        let bounds = std::iter::once(0).chain(t.text.match_indices(['=', ':']).map(|(i, _)| i + 1));
        let found = bounds
            .flat_map(|b| [b, b + wrapper_len(&t.text[b..])])
            .find_map(|at| match_prefixed(&t.text[at..]).map(|m| (at, m)));
        let Some((at, (kind, len))) = found else {
            continue;
        };
        let start = t.start + char_offset(t.text, at);
        hits.push(Hit {
            kind,
            start,
            end: start + len,
        });
        claimed.claim(start..start + len);
    }
}

fn url_credentials(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for t in toks {
        if claimed.overlaps(&(t.start..t.end())) {
            continue;
        }
        let Some(at) = userinfo_password_scheme(t.text) else {
            continue;
        };
        let start = t.start + char_offset(t.text, at);
        let end = t.start + t.text.trim_end_matches(['*', '|']).chars().count();
        hits.push(Hit {
            kind: "url_credentials",
            start,
            end,
        });
        claimed.claim(start..end);
    }
}

/// Byte offset of the URL scheme when the token's authority holds a real password.
fn userinfo_password_scheme(token: &str) -> Option<usize> {
    let sep = token.find("://")?;
    let rest = &token[sep + 3..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (userinfo, _) = authority.rsplit_once('@')?;
    let (_, password) = userinfo.split_once(':')?;
    if password.is_empty() || is_placeholder(password) {
        return None;
    }
    let scheme = |c: char| alnum(c) || matches!(c, '+' | '.' | '-');
    Some(
        token[..sep]
            .char_indices()
            .rev()
            .find(|(_, c)| !scheme(*c))
            .map_or(0, |(i, c)| i + c.len_utf8()),
    )
}

const KEYWORDS: &[&str] = &[
    "password",
    "pgpassword",
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

/// camelCase boundaries count as word boundaries: `accessToken` -> `access_token`.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn names_keyword(name: &str, keywords: &[&str]) -> bool {
    let snake = snake_case(name);
    keywords.iter().any(|k| {
        snake.strip_suffix(k).is_some_and(|head| {
            head.chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        })
    })
}

pub(super) fn is_credential_name(name: &str) -> bool {
    names_keyword(name, KEYWORDS)
}

fn is_placeholder(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    lower.starts_with(['<', '$'])
        || lower.starts_with("{{")
        || lower.starts_with("***")
        || (lower.len() > 2 && lower.starts_with('%') && lower.ends_with('%'))
        || matches!(
            lower.as_str(),
            "redacted" | "changeme" | "placeholder" | "null" | "none" | "hidden" | "example"
        )
        || lower.chars().all(|c| c == 'x')
}

/// Values that point at a secret (or are not one) instead of being one.
fn is_reference(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    ["process.env.", "env.", "secrets."]
        .iter()
        .any(|p| lower.starts_with(p))
        || is_path(v)
        || is_iso_date(v)
}

fn is_path(v: &str) -> bool {
    let drive = matches!(v.as_bytes(), [d, b':', b'/' | b'\\', ..] if d.is_ascii_alphabetic());
    v.starts_with(['/', '\\']) || ["~/", "./", "../"].iter().any(|p| v.starts_with(p)) || drive
}

fn is_iso_date(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() >= 10
        && b[..10].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
        && (b.len() == 10 || b[10] == b'T')
}

fn digit_or_mixed(v: &str) -> bool {
    v.chars().any(|c| c.is_ascii_digit())
        || (v.chars().any(char::is_uppercase) && v.chars().any(char::is_lowercase))
}

/// Whether `v` has the shape of a credential value (not a placeholder or a reference).
pub(super) fn is_credential_value(v: &str) -> bool {
    v.chars().count() >= 8
        && !v.chars().any(char::is_whitespace)
        && (digit_or_mixed(v) || v.chars().count() >= 20)
        && !is_placeholder(v)
        && !is_reference(v)
}

struct Slot {
    tok: usize,
    off: usize,
    /// The key sat in a `?k=v&k=v` run, where `&` and `#` end the value.
    query: bool,
}

/// Byte offset of the first `=` / `:` whose key segment names a credential.
///
/// A key segment starts after the previous `? & / # = :`, which keeps the scan linear.
fn credential_sep(raw: &str) -> Option<(usize, bool)> {
    let mut key_start = 0;
    let mut query = false;
    for (p, c) in raw.char_indices() {
        match c {
            '?' | '&' => {
                key_start = p + 1;
                query = true;
            }
            '/' | '#' | '=' | ':' => {
                if matches!(c, '=' | ':') && is_credential_name(&raw[key_start..p]) {
                    return Some((p, query));
                }
                key_start = p + 1;
                query = false;
            }
            _ => {}
        }
    }
    None
}

/// Where the value of a keyword at token `i` starts.
fn value_slot(toks: &[Tok], i: usize) -> Option<Slot> {
    let raw = toks[i].raw;
    let slot = |tok, off| Slot {
        tok,
        off,
        query: false,
    };
    if let Some((p, query)) = credential_sep(raw) {
        return Some(if raw.len() > p + 1 {
            Slot {
                tok: i,
                off: p + 1,
                query,
            }
        } else {
            slot(i + 1, 0)
        });
    }
    if raw.contains(['=', ':']) {
        return None;
    }
    if raw.eq_ignore_ascii_case("bearer") {
        // The caller skips a "bearer" value to the next token.
        return Some(slot(i, 0));
    }
    if !is_credential_name(raw) {
        return None;
    }
    let next = toks.get(i + 1)?.raw;
    if !next.starts_with(['=', ':']) {
        return None;
    }
    Some(if next.len() > 1 {
        slot(i + 1, 1)
    } else {
        slot(i + 2, 0)
    })
}

fn credential_fields(s: &str, toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for i in 0..toks.len() {
        let Some(Slot {
            tok: mut at,
            mut off,
            mut query,
        }) = value_slot(toks, i)
        else {
            continue;
        };
        let bearer_value = |t: &Tok, off: usize| {
            t.raw[off..]
                .trim_end_matches(TRAILING)
                .eq_ignore_ascii_case("bearer")
        };
        if toks.get(at).is_some_and(|t| bearer_value(t, off)) {
            (at, off, query) = (at + 1, 0, false);
        }
        let Some(t) = toks.get(at) else {
            continue;
        };
        let mut value = &t.raw[off..];
        if query {
            value = value.split(['&', '#']).next().unwrap_or_default();
        }
        let value = value.trim_end_matches(TRAILING);
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
    let mut counts = [0_u32; 256];
    for b in s.bytes() {
        counts[usize::from(b)] += 1;
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

fn entropy_char(c: char) -> bool {
    alnum(c) || matches!(c, '+' | '/' | '=' | '_' | '-')
}

fn looks_high_entropy(t: &str) -> bool {
    t.len() >= 32
        && t.chars().all(entropy_char)
        && t.chars().filter(char::is_ascii_digit).count() >= 3
        && t.chars().any(|c| c.is_ascii_uppercase())
        && t.chars().any(|c| c.is_ascii_lowercase())
        && entropy(t) >= 4.5
}

fn high_entropy(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &Claimed) {
    for t in toks {
        let body = t.text.trim_matches(|c| !entropy_char(c));
        let lead = t.text.chars().take_while(|c| !entropy_char(*c)).count();
        let range = t.start + lead..t.start + lead + body.chars().count();
        if looks_high_entropy(body) && !claimed.overlaps(&range) {
            hits.push(Hit {
                kind: "high_entropy",
                start: range.start,
                end: range.end,
            });
        }
    }
}
