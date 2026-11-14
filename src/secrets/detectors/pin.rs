//! PIN codes: a pin-like keyword with a short digit run near it.
//!
//! "pin" is also a verb ("pin the timeout to 3000 ms") and the digits next to it are often a
//! port, a version or an issue number, so a keyword alone is not enough. The digits count
//! when the keyword reads as a noun (shouted or a compound, attached to `:` / `=` / `-` / `#`, after a
//! determiner, before a linking word) or when the run is long enough that nothing else is
//! likely (6 digits or more).

use std::ops::Range;

use super::{Claimed, Hit, Tok, is_iso_date, names_keyword};

const KEYWORDS: &[&str] = &[
    "pin",
    "pins",
    "pincode",
    "pin_code",
    "pin-code",
    "passcode",
    "passcodes",
    "пин",
    "пины",
    "пинкод",
    "пин_код",
    "пин-код",
];

/// Tokens after the keyword that may hold the digits: "PIN as of 2026-10-18 22:15 is now <digits>".
/// Dates and times do not count.
const WINDOW: usize = 6;
/// Longest keyword token or prefix worth looking at; keeps a megabyte of `=` linear.
const MAX_NAME: usize = 32;
const MIN_DIGITS: usize = 4;
const MAX_DIGITS: usize = 12;
/// A run this long is taken for a PIN without further evidence.
const SELF_EVIDENT_DIGITS: usize = 6;
/// Markdown emphasis and table pipes around a value; `#4821` is an issue number, not a PIN.
const WRAPPERS: [char; 4] = ['*', '_', '|', '~'];
const SEPARATORS: [char; 4] = ['=', ':', '#', '-'];
const DETERMINERS: &[&str] = &[
    "the",
    "a",
    "an",
    "my",
    "your",
    "his",
    "her",
    "its",
    "our",
    "their",
    "this",
    "that",
    "new",
    "old",
    "current",
    "wrong",
    "same",
    "default",
    "temporary",
    "offline",
    "unlock",
    "parent",
    "admin",
    "master",
    "backup",
    "device",
    "phone",
    "card",
    "bank",
    "sim",
    "user",
    "family",
];
const LINKING: &[&str] = &[
    "is", "was", "are", "were", "as", "now", "set", "changed", "equals", "number", "no", "code",
    "value", "=",
];
/// The number after one of these is a port, a version or an identifier.
const NOT_A_PIN_AFTER: &[&str] = &[
    "port", "ports", "version", "v", "issue", "build", "node", "pr", "ticket", "task", "bug",
    "line", "step",
];
const UNITS: &[&str] = &[
    "ms",
    "s",
    "sec",
    "secs",
    "second",
    "seconds",
    "min",
    "mins",
    "minute",
    "minutes",
    "h",
    "hr",
    "hrs",
    "hour",
    "hours",
    "d",
    "day",
    "days",
    "rows",
    "row",
    "items",
    "bytes",
    "kb",
    "mb",
    "gb",
    "tb",
    "kib",
    "mib",
    "gib",
    "%",
    "px",
    "times",
    "chars",
    "characters",
    "lines",
    "files",
    "requests",
    "users",
    "entries",
    "records",
];

pub(in crate::secrets) fn is_name(name: &str) -> bool {
    let name = name.trim_matches(|c: char| !c.is_alphanumeric());
    names_keyword(name, KEYWORDS) || names_keyword(&name.to_lowercase(), KEYWORDS)
}

/// Groups of digits split by spaces or dashes ("4829 1736"): 4 to 12 digits in all, and not
/// a year (19xx / 20xx) or a date, which sit next to "pin" in notes and versions.
pub(in crate::secrets) fn is_value(v: &str) -> bool {
    let groups: Vec<&str> = v.split([' ', '-']).collect();
    if groups
        .iter()
        .any(|g| g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let digits: usize = groups.iter().map(|g| g.len()).sum();
    let date_like = groups[0].len() == 4
        && (groups[0].starts_with("19") || groups[0].starts_with("20"))
        && groups[1..].iter().all(|g| g.len() == 2);
    (MIN_DIGITS..=MAX_DIGITS).contains(&digits) && !date_like
}

/// One place a value may start: the text after a separator inside the keyword token, or a token.
struct Piece<'a> {
    text: &'a str,
    start: usize,
    raw: &'a str,
}

/// How a token reads as the keyword: its name, and the value glued to it (`PIN-4829`).
struct Split<'a> {
    name: &'a str,
    inline: &'a str,
    inline_at: usize,
    attached: bool,
}

struct Run {
    last: usize,
    range: Range<usize>,
    digits: usize,
}

pub(super) fn scan(toks: &[Tok], hits: &mut Vec<Hit>, claimed: &mut Claimed) {
    for (i, key) in toks.iter().enumerate() {
        for split in splits(key) {
            let noun = split.attached
                || never_a_verb(split.name)
                || after_determiner(toks, i)
                || before_linking_word(toks, i);
            let pieces = pieces(toks, i, &split);
            if let Some(range) = find(&pieces, noun, claimed) {
                hits.push(Hit {
                    kind: "pin_code",
                    start: range.start,
                    end: range.end,
                });
                claimed.claim(range);
                break;
            }
        }
    }
}

fn splits<'a>(key: &Tok<'a>) -> Vec<Split<'a>> {
    let text = key.text;
    let mut out = Vec::new();
    if text.len() <= MAX_NAME && is_name(text) {
        out.push(Split {
            name: text,
            inline: "",
            inline_at: 0,
            attached: key.raw.ends_with(SEPARATORS),
        });
    }
    for (at, sep) in text
        .match_indices(SEPARATORS)
        .take_while(|(at, _)| *at <= MAX_NAME)
    {
        let name = &text[..at];
        // `-` and `#` glue a number on ("PIN-4829"); before a word they are hyphenation
        // ("pin-point", "pin-up").
        let digits_follow = text[at + 1..].starts_with(|c: char| c.is_ascii_digit());
        if is_name(name) && (digits_follow || !matches!(sep, "-" | "#")) {
            out.push(Split {
                name,
                inline: &text[at + 1..],
                inline_at: key.start + name.chars().count() + 1,
                attached: true,
            });
        }
    }
    out
}

/// An acronym ("PIN", "ПИН") is the noun, and so is a compound ("pincode", "passcode").
fn never_a_verb(name: &str) -> bool {
    let letters: Vec<char> = name.chars().filter(|c| c.is_alphabetic()).collect();
    let word = name.to_lowercase();
    (letters.len() >= 3 && letters.iter().all(|c| c.is_uppercase()))
        || word.contains("code")
        || word.contains("код")
}

fn word(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn after_determiner(toks: &[Tok], i: usize) -> bool {
    i > 0 && DETERMINERS.contains(&word(toks[i - 1].text).as_str())
}

fn before_linking_word(toks: &[Tok], i: usize) -> bool {
    toks[i + 1..]
        .iter()
        .find(|t| !t.text.is_empty())
        .is_some_and(|t| LINKING.contains(&t.text.to_lowercase().as_str()))
}

fn ends_sentence(raw: &str) -> bool {
    raw.ends_with(['.', '!', '?'])
}

fn is_time(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|p| (1..=2).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

/// What may hold the value: the glued text, then the tokens that follow within the window and
/// the sentence, without dates and times.
fn pieces<'a>(toks: &'a [Tok<'a>], i: usize, split: &Split<'a>) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    if !split.inline.is_empty() {
        out.push(Piece {
            text: split.inline,
            start: split.inline_at,
            raw: toks[i].raw,
        });
    }
    let mut over = ends_sentence(toks[i].raw);
    let mut counted = 0;
    for t in toks[i + 1..].iter().filter(|t| !t.text.is_empty()) {
        if over || counted == WINDOW {
            break;
        }
        over = ends_sentence(t.raw);
        if is_iso_date(t.text) || is_time(t.text) {
            continue;
        }
        counted += 1;
        out.push(Piece {
            text: t.text,
            start: t.start,
            raw: t.raw,
        });
    }
    out
}

/// The body of a piece when it is digits (or digit groups joined by `-`), and the chars before it.
fn digits_of<'a>(p: &Piece<'a>) -> Option<(usize, &'a str)> {
    let body = p.text.trim_matches(WRAPPERS);
    let lead = p.text.chars().take_while(|c| WRAPPERS.contains(c)).count();
    body.split('-')
        .all(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()))
        .then_some((lead, body))
}

fn count_digits(s: &str) -> usize {
    s.bytes().filter(u8::is_ascii_digit).count()
}

/// The digits starting at piece `j`, with the groups that follow them ("1234 5678").
fn digit_run(pieces: &[Piece], j: usize) -> Option<Run> {
    let (lead, body) = digits_of(&pieces[j])?;
    let start = pieces[j].start + lead;
    let (mut last, mut end) = (j, start + body.len());
    let mut joined = body.to_owned();
    let mut digits = count_digits(body);
    while let Some(next) = pieces.get(last + 1) {
        let Some((lead, body)) = digits_of(next) else {
            break;
        };
        if ends_sentence(pieces[last].raw) || digits + count_digits(body) > MAX_DIGITS {
            break;
        }
        joined.push(' ');
        joined.push_str(body);
        digits += count_digits(body);
        last += 1;
        end = next.start + lead + body.len();
    }
    is_value(&joined).then_some(Run {
        last,
        range: start..end,
        digits,
    })
}

fn find(pieces: &[Piece], noun: bool, claimed: &Claimed) -> Option<Range<usize>> {
    let mut j = 0;
    while j < pieces.len() {
        let Some(run) = digit_run(pieces, j) else {
            j += 1;
            continue;
        };
        let evident = noun || run.digits >= SELF_EVIDENT_DIGITS;
        if evident && !other_number(pieces, j, run.last) && !claimed.overlaps(&run.range) {
            return Some(run.range);
        }
        j = run.last + 1;
    }
    None
}

/// A port, a version, an issue number or a quantity rather than a PIN.
fn other_number(pieces: &[Piece], first: usize, last: usize) -> bool {
    let before = first
        .checked_sub(1)
        .map(|k| word(pieces[k].text))
        .is_some_and(|w| NOT_A_PIN_AFTER.contains(&w.as_str()));
    let unit = pieces
        .get(last + 1)
        .is_some_and(|p| UNITS.contains(&p.text.to_lowercase().as_str()));
    let port_after = pieces[last + 1..]
        .iter()
        .take(3)
        .any(|p| matches!(word(p.text).as_str(), "port" | "ports"));
    before || unit || port_after
}
