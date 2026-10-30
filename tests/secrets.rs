//! Scanner thresholds, offsets and the refusal text. Every secret here is synthetic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::assert_is_empty)]

use serde_json::json;
use yrem::Error;
use yrem::secrets::{Finding, guard, refusal, scan};

const ALNUM: &str = "aB3dE5gH7jK9mN2pQ4sT6vW8yZ1bC0eF";

fn text(s: &str) -> Vec<Finding> {
    scan(&json!({ "text": s }))
}

fn kinds(s: &str) -> Vec<&'static str> {
    text(s).iter().map(|f| f.kind).collect()
}

fn only(s: &str) -> Finding {
    let mut f = text(s);
    assert_eq!(f.len(), 1, "{s:?}: {f:?}");
    f.remove(0)
}

fn threshold(kind: &str, prefix: &str, fill: char, min: usize) {
    let at = |n: usize| format!("{prefix}{}", fill.to_string().repeat(n));
    assert_eq!(kinds(&at(min)), vec![kind.to_owned()], "{kind} at min");
    assert!(kinds(&at(min - 1)).is_empty(), "{kind} below min");
}

#[test]
fn aws_key_is_exactly_sixteen_after_prefix() {
    threshold("aws_access_key", "AKIA", 'Z', 16);
    threshold("aws_access_key", "ASIA", '7', 16);
    assert!(kinds(&format!("AKIA{}", "Z".repeat(17))).is_empty());
    assert!(kinds(&format!("AKIA{}", "z".repeat(16))).is_empty());
}

#[test]
fn github_tokens() {
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        threshold("github_token", p, 'a', 36);
    }
    threshold("github_token", "github_pat_", 'a', 22);
    assert_eq!(kinds(&format!("github_pat_{}", "a_".repeat(15))).len(), 1);
}

#[test]
fn gitlab_slack_and_openai_keys() {
    threshold("gitlab_token", "glpat-", 'a', 20);
    for p in ["xoxa-", "xoxb-", "xoxp-", "xoxr-", "xoxs-"] {
        threshold("slack_token", p, '1', 10);
    }
    let at = |n: usize| format!("sk-{}", ALNUM.chars().take(n).collect::<String>());
    assert_eq!(kinds(&at(20)), vec!["openai_anthropic_key"]);
    assert!(kinds(&at(19)).is_empty());
    assert_eq!(kinds("sk-ant-api03-abcdefghij-klmnopqrst_uv").len(), 1);
    assert_eq!(kinds(&format!("sk-proj-{ALNUM}")).len(), 1);
}

#[test]
fn stripe_keys() {
    for p in ["sk_live_", "sk_test_", "rk_live_", "rk_test_"] {
        threshold("stripe_key", p, 'a', 10);
    }
    threshold("stripe_key", "whsec_", 'a', 20);
}

#[test]
fn google_key_is_exactly_thirty_five_after_prefix() {
    let at = |n: usize| format!("AIza{}", "a".repeat(n));
    assert!(kinds(&at(34)).is_empty());
    assert_eq!(kinds(&at(35)), vec!["google_api_key"]);
    assert!(kinds(&at(36)).is_empty());
    assert_eq!(kinds(&format!("AIza{}-_", "a".repeat(33))).len(), 1);
}

#[test]
fn npm_huggingface_and_age_tokens() {
    threshold("npm_token", "npm_", 'a', 36);
    threshold("huggingface_token", "hf_", 'a', 30);
    threshold("age_secret_key", "AGE-SECRET-KEY-1", 'Q', 50);
}

#[test]
fn ydb_token() {
    threshold("ydb_token", "ydb_", 'a', 32);
    assert_eq!(only(&format!("ydb_{}", "a".repeat(64))).kind, "ydb_token");
}

#[test]
fn jwt_needs_three_segments_and_a_long_signature() {
    let jwt = |sig: usize| format!("eyJhbGciOi.eyJzdWIiOiIx.{}", "s".repeat(sig));
    assert_eq!(kinds(&jwt(16)), vec!["jwt"]);
    assert!(kinds(&jwt(15)).is_empty());
    assert!(kinds("eyJhbGciOi.eyJzdWIiOiIx").is_empty());
    assert!(kinds(&format!("abc.eyJzdWIiOiIx.{}", "s".repeat(20))).is_empty());
    assert!(kinds(&format!("eyJhbGciOi.abc.{}", "s".repeat(20))).is_empty());
    assert!(kinds(&format!("{}.x", jwt(20))).is_empty());
    let f = only(&format!("**eyJhbGciOi.eyJzdWIiOiIx.{}**", "s".repeat(20)));
    assert_eq!(
        (f.kind, f.start, f.end),
        ("jwt", 2, 2 + 10 + 1 + 12 + 1 + 20)
    );
}

#[test]
fn jwt_in_query_string_measures_only_its_own_run() {
    let jwt = format!("eyJhbGciOi.eyJzdWIiOiIx.{}", "s".repeat(20));
    let f = only(&format!("https://h/cb?id_token={jwt}&state=abc"));
    assert_eq!((f.kind, f.start), ("jwt", 22));
    assert_eq!(f.end - f.start, jwt.len());
}

#[test]
fn private_key_spans_the_whole_block() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIE\nabcd\n-----END RSA PRIVATE KEY-----";
    let f = only(&format!("key: {pem} tail"));
    assert_eq!((f.kind, f.start, f.end), ("private_key", 5, 5 + pem.len()));
    let open = "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA";
    assert_eq!(only(open).end, open.len());
    assert_eq!(
        kinds("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").len(),
        1
    );
}

#[test]
fn certificates_and_public_keys_are_not_flagged() {
    assert!(kinds("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----").is_empty());
    assert!(kinds("-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----").is_empty());
    assert!(kinds("-----BEGIN ").is_empty());
}

#[test]
fn two_private_keys_are_two_findings() {
    let pem = "-----BEGIN PRIVATE KEY-----\nAA\n-----END PRIVATE KEY-----";
    assert_eq!(kinds(&format!("{pem}\n{pem}")).len(), 2);
    let both = format!("-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----\n{pem}");
    assert_eq!(kinds(&both), vec!["private_key"]);
}

#[test]
fn url_credentials() {
    assert_eq!(
        kinds("postgres://app:s3cr3t@db.local:5432/x"),
        vec!["url_credentials"]
    );
    assert_eq!(
        kinds("see https://u:p@host?x=1 now"),
        vec!["url_credentials"]
    );
    for ok in [
        "git@github.com:avbel/yrem.git",
        "https://user@host/path",
        "http://host:8080/a",
        "mailto:me@example.com",
        "https://u:@host",
        "https://host/path?u=a:b@c",
    ] {
        assert!(kinds(ok).is_empty(), "{ok}");
    }
}

#[test]
fn credential_value_length_and_shape() {
    assert!(kinds("password=Abcdef1").is_empty());
    assert_eq!(kinds("password=Abcdef12"), vec!["credential_field"]);
    assert!(kinds(&format!("password={}", "a".repeat(19))).is_empty());
    assert_eq!(kinds(&format!("password={}", "a".repeat(20))).len(), 1);
    assert!(kinds("password=abcdefgh").is_empty());
    assert_eq!(kinds("password=abcdefGH").len(), 1);
    assert_eq!(kinds("password=abcdefg1").len(), 1);
}

#[test]
fn credential_field_forms() {
    for s in [
        "password: Abcdef12",
        "password =Abcdef12",
        "password :Abcdef12",
        "password = Abcdef12",
        "password => Abcdef12",
        "\"password\": \"Abcdef12\"",
        "DB_PASSWORD=Abcdef12",
        "API-KEY: Abcdef12",
        "Authorization: Bearer Abcdef12",
        "Bearer Abcdef12",
        "bearer abcdefgh1.",
        "authorization: Abcdef12",
        "apiKey=Abcdef12",
        "token:Abcdef12",
        "x-auth-token: Abcdef12",
    ] {
        assert_eq!(kinds(s), vec!["credential_field"], "{s}");
    }
}

#[test]
fn credential_field_near_misses() {
    for s in [
        "key=Abcdef12",
        "pass=Abcdef12",
        "auth=Abcdef12",
        "mypassword=Abcdef12",
        "password_hash=Abcdef12",
        "password is Abcdef12",
        "password:",
        "password",
        "password =",
        "password Abcdef12",
        "Bearer",
        "Authorization: Bearer",
        "password=has space1",
    ] {
        assert!(kinds(s).is_empty(), "{s}");
    }
}

#[test]
fn placeholders_are_not_credentials() {
    for v in [
        "<redacted-value>",
        "$DATABASE_PASSWORD",
        "{{ secrets.PASSWORD }}",
        "{{PASSWORD1}}",
        "***********",
        "REDACTED",
        "changeme",
        "Placeholder",
        "xxxxxxxxxx",
        "XXXXXXXXXXXX",
        "<12345678>",
    ] {
        assert!(kinds(&format!("password={v}")).is_empty(), "{v}");
        assert!(kinds(&format!("password: \"{v}\"")).is_empty(), "{v}");
        assert!(scan(&json!({"password": v})).is_empty(), "{v}");
    }
}

#[test]
fn metadata_keys() {
    let f = scan(&json!({"memories": [{}, {"metadata": {"db": {"password": "Abcdef12"}}}]}));
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].path, "/memories/1/metadata/db/password");
    assert_eq!((f[0].start, f[0].end), (0, 8));
    assert_eq!(
        scan(&json!({"metadata": {"DB_Token": "abcdefgh1"}})).len(),
        1
    );
    assert!(scan(&json!({"metadata": {"password": "short1"}})).is_empty());
    assert!(scan(&json!({"metadata": {"password": 12_345_678}})).is_empty());
    assert!(scan(&json!({"metadata": {"password": "abc def 123456"}})).is_empty());
    assert!(scan(&json!({"metadata": {"owner": "Abcdef12"}})).is_empty());
    let aws = format!("AKIA{}", "Z".repeat(16));
    let f = scan(&json!({"metadata": {"aws_secret_access_key": aws}}));
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, "aws_access_key");
}

#[test]
fn pointer_escapes_keys() {
    let f = scan(&json!({"metadata": {"a/b~c": {"password": "Abcdef12"}}}));
    assert_eq!(f[0].path, "/metadata/a~1b~0c/password");
}

#[test]
fn entropy_threshold_and_charset() {
    assert_eq!(only(ALNUM).kind, "high_entropy");
    assert!(kinds(&ALNUM[..31]).is_empty());
    assert!(kinds(&"0123456789abcdef".repeat(4)).is_empty());
    assert!(kinds("019a1b2c-3d4e-7f50-8a6b-7c8d9e0f1a2b").is_empty());
    assert!(kinds("019A1b2c-3D4e-7f50-8A6b-7c8d9e0f1a2b").is_empty());
    assert!(kinds("/Users/avbel/Projects/yrem/src/ops/mod").is_empty());
    assert!(kinds("someVeryLongCamelCaseIdentifierForTheThing").is_empty());
    assert!(kinds("a_very_long_snake_case_identifier_name_v2").is_empty());
    assert!(kinds(&format!("{ALNUM}!")).len() <= 1);
    assert!(kinds(&ALNUM.replace('a', "é")).is_empty());
    let no_digits = "aBcDeFgHjKmNpQsTvWyZbCeFgHjKmNpQ";
    assert!(kinds(no_digits).is_empty());
    assert!(kinds(&ALNUM.to_lowercase()).is_empty());
    assert!(kinds(&ALNUM.to_uppercase()).is_empty());
    let two_digits = "aBcDeFgHjKmNpQsTvWyZbCeFgHjKmN12";
    assert!(kinds(two_digits).is_empty());
}

#[test]
fn entropy_is_skipped_for_identifier_pointers() {
    for p in ["idempotency_key", "rid", "source_memory_rid"] {
        assert!(scan(&json!({ p: ALNUM })).is_empty(), "{p}");
    }
    assert!(scan(&json!({"metadata": {"corrects": ALNUM}})).is_empty());
    assert_eq!(scan(&json!({"metadata": {"other": ALNUM}})).len(), 1);
    assert_eq!(scan(&json!({"domain": ALNUM})).len(), 1);
    let aws = format!("AKIA{}", "Z".repeat(16));
    assert_eq!(scan(&json!({"rid": aws})).len(), 1);
}

#[test]
fn matched_token_is_reported_once() {
    assert_eq!(kinds(&format!("token=ghp_{ALNUM}abcd")).len(), 1);
    assert_eq!(kinds(&format!("sk-{ALNUM}")).len(), 1);
    assert_eq!(
        kinds(&format!("password: {ALNUM}")),
        vec!["credential_field"]
    );
    assert_eq!(
        kinds(&format!("Authorization: Bearer ghp_{ALNUM}abcd")),
        vec!["github_token"]
    );
}

#[test]
fn offsets_are_chars_not_bytes() {
    let aws = format!("AKIA{}", "Z".repeat(16));
    let f = only(&format!("Привет 👋 {aws} конец"));
    assert_eq!((f.start, f.end), (9, 29));
    let f = only(&format!("ключ:{aws}"));
    assert_eq!((f.start, f.end), (5, 25));
    let f = only("password: 🔑Абвгд123");
    assert_eq!((f.start, f.end), (10, 19));
    let f = only("日本語 password=Abcdef12 終");
    assert_eq!((f.start, f.end), (13, 21));
}

#[test]
fn offsets_inside_a_token() {
    let aws = format!("AKIA{}", "Z".repeat(16));
    let f = only(&format!("AWS_KEY={aws}."));
    assert_eq!((f.start, f.end), (8, 28));
    let f = only(&format!("x ({aws}), y"));
    assert_eq!((f.start, f.end), (3, 23));
}

#[test]
fn clean_prose_is_clean() {
    for s in [
        "",
        "Payments service uses PostgreSQL 16 on port 5432",
        "The token bucket has a capacity of 100 and refills 10 per second.",
        "Don't store the password in the repo; use a secret manager.",
        "see /v1/remember and https://example.com/docs#secret",
        "task-1234567890abcdefghij and disk-1234567890abcdefghij",
        "Привет мир",
    ] {
        assert!(kinds(s).is_empty(), "{s}");
    }
    assert!(scan(&json!({"n": 1, "b": true, "x": null, "a": [1, "ok"]})).is_empty());
}

fn finding(kind: &'static str, path: &str) -> Finding {
    Finding {
        kind,
        path: path.into(),
        start: 41,
        end: 93,
    }
}

#[test]
fn soft_refusal_text() {
    let msg = refusal("remember", &[finding("openai_anthropic_key", "/text")]);
    assert_eq!(
        msg,
        "remember refused: possible secret(s), nothing stored. openai_anthropic_key at \
         /text[41..93]. Redact them (e.g. \"<redacted>\") and retry; if this is not secret \
         data, retry with force=true (CLI: --force)."
    );
}

#[test]
fn guard_error_properties() {
    let aws = format!("AKIA{}", "Z".repeat(16));
    let req = json!({"text": format!("key {aws} end")});
    let err = guard("remember", false, &req).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("force=true") && msg.contains("aws_access_key at /text[4..24]"));
    assert!(!msg.contains(&aws) && !msg.contains('\n'));
    assert_eq!(err.exit_code(), 7);
    assert!(!err.is_retryable());
    assert!(
        matches!(err, Error::Sensitive { ref op, ref findings } if op == "remember" && findings.len() == 1)
    );
    guard("remember", true, &req).unwrap();
    guard("remember", false, &json!({"text": "fine"})).unwrap();
}

#[test]
fn independent_findings_in_one_string_are_all_reported() {
    let aws = format!("AKIA{}", "Z".repeat(16));
    assert_eq!(
        kinds(&format!("{aws} password=Abcdef12 postgres://u:p@h/d")),
        vec!["aws_access_key", "url_credentials", "credential_field"]
    );
    let pem = "-----BEGIN PRIVATE KEY-----\nAA\n-----END PRIVATE KEY-----";
    let f = text(&format!(
        "some long preamble text before {pem} password=Abcdef12"
    ));
    assert_eq!(
        f.iter().map(|f| f.kind).collect::<Vec<_>>(),
        vec!["private_key", "credential_field"]
    );
    assert_eq!((f[0].start, f[0].end), (31, 31 + pem.len()));
}

#[test]
fn refusal_lists_five_and_counts_the_rest() {
    let many: Vec<Finding> = (0..8).map(|i| finding("jwt", &format!("/{i}"))).collect();
    let msg = refusal("remember", &many);
    assert!(msg.contains("/4[41..93]; +3 more."), "{msg}");
    assert!(!msg.contains("/5["), "{msg}");
    let five = refusal("remember", &many[..5]);
    assert!(five.contains("/4[41..93]."), "{five}");
    assert!(!five.contains("more"), "{five}");
    let two = refusal("correct", &many[..2]);
    assert!(
        two.starts_with("correct refused: possible secret(s), nothing stored. jwt at /0[41..93]; jwt at /1[41..93]. Redact"),
        "{two}"
    );
}

#[test]
fn force_waives_every_detector() {
    let pem = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----";
    let req = json!({
        "text": format!("a ydb_{} b {pem}", "k".repeat(64)),
        "metadata": {"password": "Abcdef12"}
    });
    assert_eq!(guard("remember", false, &req).unwrap_err().exit_code(), 7);
    guard("remember", true, &req).unwrap();
}

#[test]
fn findings_serialize_without_hardness() {
    let f = finding("ydb_token", "/text");
    assert_eq!(
        serde_json::to_value(&f).unwrap(),
        json!({"kind": "ydb_token", "path": "/text", "start": 41, "end": 93})
    );
}

#[test]
fn slack_token_charset_is_alnum_and_dash() {
    assert_eq!(kinds("xoxb-12345-6789").len(), 1);
    assert!(kinds("xoxb-123456789_1").is_empty());
}

fn b64(seed: u64, len: usize) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(A[usize::try_from(x >> 58).unwrap()])
        })
        .collect()
}

fn ghp() -> String {
    format!("ghp_{ALNUM}abcd")
}

#[test]
fn wrapped_secrets_are_found_with_exact_offsets() {
    let aws = format!("AKIA{}", "Z".repeat(16));
    let jwt = format!("eyJhbGciOi.eyJzdWIiOiIx.{}", "s".repeat(20));
    let sk = format!("sk-proj-{ALNUM}");
    let cases = [
        (ghp(), "github_token"),
        (aws, "aws_access_key"),
        (sk, "openai_anthropic_key"),
        (jwt, "jwt"),
        (ALNUM.to_owned(), "high_entropy"),
    ];
    for (secret, kind) in cases {
        let n = secret.chars().count();
        for (open, close) in [
            ("`", "`"),
            ("**", "**"),
            ("|", "|"),
            ("#", ""),
            ("\u{200b}", ""),
            ("(`", "`)"),
        ] {
            let f = only(&format!("x {open}{secret}{close} y"));
            let start = 2 + open.chars().count();
            assert_eq!(
                (f.kind, f.start, f.end),
                (kind, start, start + n),
                "{open}{kind}"
            );
        }
    }
}

#[test]
fn wrapped_credentials_and_urls() {
    let f = only("password: `Abcdef12`");
    assert_eq!((f.start, f.end), (11, 19));
    let f = only("see `postgres://u:s3cr3t@h/db` now");
    assert_eq!((f.kind, f.start, f.end), ("url_credentials", 5, 29));
    let f = only("**https://u:s3cr3t@h/db**");
    assert_eq!((f.kind, f.start, f.end), ("url_credentials", 2, 23));
    let f = only(&format!("token=`{}`", ghp()));
    assert_eq!((f.kind, f.start, f.end), ("github_token", 7, 47));
}

#[test]
fn query_string_credentials() {
    for (s, start, end) in [
        ("https://h/x?api_key=Abcdef12", 20, 28),
        ("https://h/x?auth_token=Abcdef12&page=2", 23, 31),
        ("redis://cache:6379/0?password=Abcdef12", 30, 38),
        ("user=bob&password=Abcdef12", 18, 26),
        ("a=1&token=Abcdef12#frag", 10, 18),
        ("a=1&token=Abcdef12&b=2", 10, 18),
    ] {
        let f = only(s);
        assert_eq!(
            (f.kind, f.start, f.end),
            ("credential_field", start, end),
            "{s}"
        );
    }
    assert!(kinds("https://h/x?page=Abcdef12&sort=asc").is_empty());
    assert!(kinds("https://h/x?token=abc&page=Abcdef12").is_empty());
    let f = only("password=Abc#12345&x");
    assert_eq!((f.start, f.end), (9, 20));
}

#[test]
fn camel_case_credential_names() {
    for name in [
        "accessToken",
        "refreshToken",
        "authToken",
        "apiToken",
        "clientSecret",
        "privateKey",
        "secretKey",
        "awsSecretAccessKey",
        "dbPassword",
        "AccessToken",
        "PGPASSWORD",
        "DB_Token",
    ] {
        assert_eq!(
            kinds(&format!("{name}=Abcdef12")),
            vec!["credential_field"],
            "{name}"
        );
        assert_eq!(scan(&json!({ name: "Abcdef12" })).len(), 1, "{name}");
    }
    for name in ["maxTokens", "passwordHash", "idempotencyKey", "monkey"] {
        assert!(kinds(&format!("{name}=Abcdef12")).is_empty(), "{name}");
        assert!(scan(&json!({ name: "Abcdef12" })).is_empty(), "{name}");
    }
}

#[test]
fn certificates_and_public_keys_of_real_length_are_clean() {
    let body = |seed| {
        (0..20_u64)
            .map(|i| b64(seed + i, 64))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(kinds(&b64(1, 64)), vec!["high_entropy"]);
    for label in [
        "CERTIFICATE",
        "PUBLIC KEY",
        "RSA PUBLIC KEY",
        "CERTIFICATE REQUEST",
    ] {
        let pem = format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----",
            body(7)
        );
        assert!(kinds(&pem).is_empty(), "{label}");
        assert!(kinds(&format!("cert: {pem} tail")).is_empty(), "{label}");
    }
    let unterminated = format!("-----BEGIN CERTIFICATE-----\n{}", body(9));
    assert!(kinds(&unterminated).is_empty());
    for kind in ["ssh-rsa", "ssh-ed25519", "ssh-dss", "ecdsa-sha2-nistp256"] {
        let line = format!("{kind} {} me@host", b64(3, 372));
        assert!(kinds(&line).is_empty(), "{kind}");
    }
    assert_eq!(
        kinds(&format!("ssh-rsa {} {}", b64(3, 372), b64(4, 64))).len(),
        1
    );
    let both = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n{}",
        body(5),
        ghp()
    );
    assert_eq!(kinds(&both), vec!["github_token"]);
    let inside = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n{}\n-----END CERTIFICATE-----",
        body(5),
        ghp()
    );
    assert_eq!(kinds(&inside), vec!["github_token"]);
}

#[test]
fn secrets_in_object_keys_are_found_and_never_echoed() {
    let token = ghp();
    let f = scan(&json!({ token.clone(): "fine" }));
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].kind, f[0].path.as_str(), f[0].start, f[0].end),
        ("github_token", "/<key>", 0, 40)
    );
    let f = scan(&json!({"metadata": {token.clone(): {"password": "Abcdef12"}, "ok": 1}}));
    let paths: Vec<_> = f.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["/metadata/<key>", "/metadata/<key>/password"]);
    let f = scan(&json!({ token.clone(): ["a", token.clone()] }));
    assert_eq!(f[1].path, "/<key>/1");
    let msg = guard(
        "remember",
        false,
        &json!({"metadata": {token.clone(): "v"}}),
    )
    .unwrap_err()
    .to_string();
    assert!(!msg.contains(&token), "{msg}");
    let f = scan(&json!({"password": token}));
    assert_eq!(f[0].path, "/password");
    assert!(scan(&json!({"plain_key": "fine"})).is_empty());
}

#[test]
fn pathological_input_scans_in_linear_time() {
    let start = std::time::Instant::now();
    for unit in ["=", "a=", ":", "eyJ."] {
        let big = unit.repeat(1_000_000 / unit.len());
        let _ = text(&big);
    }
    assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
}

#[test]
fn placeholders_after_vendor_prefixes() {
    for s in [
        format!("sk-{}", "x".repeat(24)),
        format!("ghp_{}", "x".repeat(36)),
        format!("ghp_{}", "X".repeat(36)),
        format!("glpat-{}", "x".repeat(20)),
        format!("AKIA{}", "X".repeat(16)),
        format!("postgres://u:{}@h/db", "*".repeat(4)),
        "postgres://u:xxxx@h/db".to_owned(),
        "postgres://u:$DB_PASS@h/db".to_owned(),
        "postgres://u:%DB_PASS%@h/db".to_owned(),
        "postgres://u:<password>@h/db".to_owned(),
        "postgres://u:${PASS}@h/db".to_owned(),
    ] {
        assert!(kinds(&s).is_empty(), "{s}");
    }
    assert_eq!(kinds("postgres://u:ab@h/db"), vec!["url_credentials"]);
}

#[test]
fn slugs_are_not_openai_keys() {
    for s in [
        "sk-payment-gateway-production",
        "sk-lowercase-words-only-here-ok",
    ] {
        assert!(kinds(s).is_empty(), "{s}");
    }
    for s in [
        "sk-proj-abcdefghijklmnopqrstuv1",
        "sk-AbcdefghijklmnopqrstuvW",
        "sk-ant-api03-abcdefghij-klmnopqrst_uv",
    ] {
        assert_eq!(kinds(s), vec!["openai_anthropic_key"], "{s}");
    }
}

#[test]
fn references_are_not_credential_values() {
    for v in [
        "process.env.DB_PASSWORD",
        "env.DB_PASSWORD",
        "secrets.DB_PASSWORD",
        "${DB_PASSWORD}",
        "$DB_PASSWORD",
        "%DB_PASSWORD%",
        "/Users/avbel/Projects/yrem",
        "~/secrets/file1",
        "./config/file1.toml",
        "../config/file1.toml",
        "C:\\Users\\avbel\\x1",
        "C:/Users/avbel/x1",
        "2026-10-30",
        "2026-10-30T12:00:00Z",
    ] {
        assert!(kinds(&format!("password={v}")).is_empty(), "{v}");
        assert!(kinds(&format!("token: {v}")).is_empty(), "{v}");
        assert!(scan(&json!({"password": v})).is_empty(), "{v}");
    }
    assert!(kinds("pwd: /Users/avbel/Projects").is_empty());
    for v in ["2026-10-30x", "2026/10/30aa", "C:Users1234", "e2026-10-30"] {
        assert_eq!(kinds(&format!("password={v}")).len(), 1, "{v}");
    }
}

#[test]
fn placeholder_runs_and_near_misses() {
    assert!(kinds(&format!("password={}", "x".repeat(20))).is_empty());
    assert!(kinds(&format!("password={}", "X".repeat(20))).is_empty());
    assert_eq!(
        kinds(&format!("password={}", "x".repeat(19) + "y")).len(),
        1
    );
    for v in [
        "<a-long-placeholder-value>",
        "$a_long_placeholder_value",
        "***a_long_placeholder_value_x",
    ] {
        assert!(kinds(&format!("password={v}")).is_empty(), "{v}");
    }
    assert!(kinds("username = Abcdef12").is_empty());
    assert!(kinds("username: Abcdef12").is_empty());
    let random36 = b64(11, 36);
    assert_eq!(only(&random36).kind, "high_entropy");
}

#[test]
fn bearer_after_a_keyword_in_the_same_token() {
    for (s, start, end) in [
        ("token=Bearer Abcdef12", 13, 21),
        ("authorization:Bearer Abcdef12", 21, 29),
        ("Bearer Abcdef12", 7, 15),
    ] {
        let f = only(s);
        assert_eq!(
            (f.kind, f.start, f.end),
            ("credential_field", start, end),
            "{s}"
        );
    }
    assert!(kinds("token=Bearer").is_empty());
}

#[test]
fn percent_placeholders_need_both_ends() {
    assert!(kinds("postgres://u:%DB_PASS%@h/db").is_empty());
    for pw in ["%%", "%abc", "abc%", "a%b"] {
        assert_eq!(
            kinds(&format!("postgres://u:{pw}@h/db")),
            vec!["url_credentials"],
            "{pw}"
        );
    }
    assert_eq!(kinds("password=%Abcdef12").len(), 1);
    assert_eq!(kinds("password=Abcdef12%").len(), 1);
}
