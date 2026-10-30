#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use yrem::config::{Config, Origin, Overrides};

static N: AtomicUsize = AtomicUsize::new(0);

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "yrem-config-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_cfg(dir: &Path, text: &str) -> PathBuf {
    let p = dir.join("config.toml");
    std::fs::write(&p, text).unwrap();
    p
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |k| m.get(k).cloned()
}

fn with_file(path: &Path) -> Overrides {
    Overrides {
        config: Some(path.to_owned()),
        ..Overrides::default()
    }
}

#[test]
fn flag_beats_env_beats_file() {
    let d = tmp();
    let f = write_cfg(&d, "url = \"http://file\"\ntoken = \"filetok\"\n");
    let e = env(&[
        ("YANTRIKDB_SERVER_URL", "http://env"),
        ("YANTRIKDB_TOKEN", "envtok"),
    ]);

    let o = Overrides {
        url: Some("http://flag".into()),
        token: Some("flagtok".into()),
        config: Some(f.clone()),
        ..Overrides::default()
    };
    let c = Config::resolve(&o, &e).unwrap();
    assert_eq!(
        (c.url.as_str(), c.token.as_deref()),
        ("http://flag", Some("flagtok"))
    );
    assert_eq!((c.url_origin, c.token_origin), (Origin::Flag, Origin::Flag));
    assert_eq!(c.file.as_deref(), Some(f.as_path()));

    let c = Config::resolve(&with_file(&f), &e).unwrap();
    assert_eq!(
        (c.url.as_str(), c.token.as_deref()),
        ("http://env", Some("envtok"))
    );
    assert_eq!((c.url_origin, c.token_origin), (Origin::Env, Origin::Env));

    let c = Config::resolve(&with_file(&f), env(&[])).unwrap();
    assert_eq!(
        (c.url.as_str(), c.token.as_deref()),
        ("http://file", Some("filetok"))
    );
    assert_eq!((c.url_origin, c.token_origin), (Origin::File, Origin::File));
}

#[test]
fn mixed_sources_per_field() {
    let d = tmp();
    let f = write_cfg(&d, "url = \"http://file\"\n");
    let o = Overrides {
        token: Some("t".into()),
        config: Some(f),
        ..Overrides::default()
    };
    let c = Config::resolve(&o, env(&[])).unwrap();
    assert_eq!(c.url_origin, Origin::File);
    assert_eq!(c.token_origin, Origin::Flag);
}

#[test]
fn token_beats_token_cmd() {
    let d = tmp();
    let f = write_cfg(
        &d,
        "url = \"http://h\"\ntoken = \"t\"\ntoken_cmd = \"printf cmd\"\n",
    );
    let c = Config::resolve(&with_file(&f), env(&[])).unwrap();
    assert_eq!(c.token.as_deref(), Some("t"));
    assert_eq!(c.token_origin, Origin::File);
}

#[test]
fn env_token_beats_token_cmd_without_running_it() {
    let d = tmp();
    let f = write_cfg(&d, "url = \"http://h\"\ntoken_cmd = \"exit 3\"\n");
    let c = Config::resolve(&with_file(&f), env(&[("YANTRIKDB_TOKEN", "e")])).unwrap();
    assert_eq!(c.token.as_deref(), Some("e"));
}

#[test]
fn missing_url_is_error() {
    let d = tmp();
    let f = write_cfg(&d, "token = \"t\"\n");
    let e = Config::resolve(&with_file(&f), env(&[])).unwrap_err();
    assert!(e.to_string().contains("no server URL"), "{e}");
}

#[test]
fn no_token_is_unset() {
    let o = Overrides {
        url: Some("http://h".into()),
        config: Some(tmp().join("absent.toml")),
        ..Overrides::default()
    };
    // Explicit missing file errors; use default path with an empty XDG dir instead.
    assert!(Config::resolve(&o, env(&[])).is_err());
    let xdg = tmp();
    let c = Config::resolve(
        &Overrides {
            url: Some("http://h".into()),
            ..Overrides::default()
        },
        env(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())]),
    )
    .unwrap();
    assert_eq!(c.token, None);
    assert_eq!(c.token_origin, Origin::Unset);
    assert_eq!(c.file, None);
}

#[test]
fn explicit_missing_file_is_error() {
    let o = Overrides {
        url: Some("http://h".into()),
        config: Some(tmp().join("nope.toml")),
        ..Overrides::default()
    };
    let e = Config::resolve(&o, env(&[])).unwrap_err();
    assert!(format!("{e:#}").contains("nope.toml"), "{e:#}");
}

#[test]
fn default_path_from_xdg() {
    let xdg = tmp();
    std::fs::create_dir_all(xdg.join("yrem")).unwrap();
    write_cfg(&xdg.join("yrem"), "url = \"http://xdg\"\n");
    let c = Config::resolve(
        &Overrides::default(),
        env(&[
            ("XDG_CONFIG_HOME", xdg.to_str().unwrap()),
            ("HOME", "/nonexistent"),
        ]),
    )
    .unwrap();
    assert_eq!(c.url, "http://xdg");
    assert_eq!(c.file, Some(xdg.join("yrem").join("config.toml")));
}

#[test]
fn default_path_from_home() {
    let home = tmp();
    let dir = home.join(".config").join("yrem");
    std::fs::create_dir_all(&dir).unwrap();
    write_cfg(&dir, "url = \"http://home\"\n");
    let c = Config::resolve(
        &Overrides::default(),
        env(&[("HOME", home.to_str().unwrap())]),
    )
    .unwrap();
    assert_eq!(c.url, "http://home");
    assert_eq!(c.file, Some(dir.join("config.toml")));
}

#[test]
fn empty_xdg_falls_back_to_home() {
    let home = tmp();
    let dir = home.join(".config").join("yrem");
    std::fs::create_dir_all(&dir).unwrap();
    write_cfg(&dir, "url = \"http://home\"\n");
    let c = Config::resolve(
        &Overrides::default(),
        env(&[("XDG_CONFIG_HOME", ""), ("HOME", home.to_str().unwrap())]),
    )
    .unwrap();
    assert_eq!(c.url, "http://home");
}

#[test]
fn unknown_key_is_error() {
    let d = tmp();
    let f = write_cfg(&d, "url = \"http://h\"\ntokn = \"x\"\n");
    let e = Config::resolve(&with_file(&f), env(&[])).unwrap_err();
    assert!(format!("{e:#}").contains("tokn"), "{e:#}");
}

#[test]
fn invalid_toml_is_error() {
    let d = tmp();
    let f = write_cfg(&d, "url = = \n");
    assert!(Config::resolve(&with_file(&f), env(&[])).is_err());
}

#[test]
fn trailing_slash_stripped() {
    for (input, want) in [
        ("http://h:1/", "http://h:1"),
        ("http://h:1///", "http://h:1"),
        ("http://h:1/p/", "http://h:1/p"),
        ("http://h:1", "http://h:1"),
    ] {
        let o = Overrides {
            url: Some(input.into()),
            config: None,
            token: None,
            ..Overrides::default()
        };
        let xdg = tmp();
        let c = Config::resolve(&o, env(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())])).unwrap();
        assert_eq!(c.url, want);
    }
}

#[test]
fn empty_strings_are_ignored() {
    let d = tmp();
    let f = write_cfg(&d, "url = \"http://file\"\ntoken = \"filetok\"\n");
    let o = Overrides {
        url: Some(String::new()),
        token: Some("   ".into()),
        config: Some(f),
        ..Overrides::default()
    };
    let e = env(&[("YANTRIKDB_SERVER_URL", ""), ("YANTRIKDB_TOKEN", " ")]);
    let c = Config::resolve(&o, e).unwrap();
    assert_eq!(c.url, "http://file");
    assert_eq!(c.token.as_deref(), Some("filetok"));
    assert_eq!((c.url_origin, c.token_origin), (Origin::File, Origin::File));
}

fn cmd_cfg(cmd: &str) -> (PathBuf, Overrides) {
    let d = tmp();
    let f = write_cfg(&d, &format!("url = \"http://h\"\ntoken_cmd = {cmd:?}\n"));
    (f.clone(), with_file(&f))
}

#[test]
fn token_cmd_success_is_trimmed() {
    let (_, o) = cmd_cfg("printf 'tok\\n'");
    let c = Config::resolve(&o, env(&[])).unwrap();
    assert_eq!(c.token.as_deref(), Some("tok"));
    assert_eq!(c.token_origin, Origin::TokenCmd);
}

#[test]
fn token_cmd_failure() {
    let (_, o) = cmd_cfg("echo oops >&2; exit 3");
    let e = Config::resolve(&o, env(&[])).unwrap_err().to_string();
    assert!(e.contains("token_cmd failed"), "{e}");
    assert!(e.contains("oops"), "{e}");
}

#[test]
fn token_cmd_empty_output() {
    let (_, o) = cmd_cfg("printf '  \\n'");
    let e = Config::resolve(&o, env(&[])).unwrap_err().to_string();
    assert!(e.contains("printed nothing"), "{e}");
}

#[test]
fn token_cmd_non_utf8() {
    let (_, o) = cmd_cfg("printf '\\377'");
    let e = Config::resolve(&o, env(&[])).unwrap_err().to_string();
    assert!(e.contains("UTF-8"), "{e}");
}

#[test]
fn debug_does_not_leak_token() {
    let o = Overrides {
        url: Some("http://h".into()),
        token: Some("super-secret-token".into()),
        config: None,
        ..Overrides::default()
    };
    let xdg = tmp();
    let c = Config::resolve(&o, env(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())])).unwrap();
    let dbg = format!("{c:?}");
    assert!(!dbg.contains("super-secret-token"), "{dbg}");
    assert!(dbg.contains("redacted"), "{dbg}");
}

#[test]
fn yrem_env_beats_yantrikdb_env_and_config_env_selects_file() {
    let d = tmp();
    let f = write_cfg(&d, "url = \"http://file\"\n");
    let fs = f.to_str().unwrap().to_owned();
    let c = Config::resolve(
        &Overrides::default(),
        env(&[
            ("YREM_URL", "http://yrem"),
            ("YANTRIKDB_SERVER_URL", "http://ydb"),
            ("YREM_TOKEN", "ytok"),
            ("YANTRIKDB_TOKEN", "dtok"),
            ("YREM_CONFIG", fs.as_str()),
        ]),
    )
    .unwrap();
    assert_eq!(c.url, "http://yrem");
    assert_eq!(c.token.as_deref(), Some("ytok"));
    assert_eq!((c.url_origin, c.token_origin), (Origin::Env, Origin::Env));
    assert_eq!(c.file.as_deref(), Some(f.as_path()));
}

#[test]
fn values_clap_took_from_env_report_env_origin() {
    let o = Overrides {
        url: Some("http://h".into()),
        token: Some("t".into()),
        url_from_env: true,
        token_from_env: false,
        ..Overrides::default()
    };
    let c = Config::resolve(&o, env(&[("XDG_CONFIG_HOME", "/nonexistent")])).unwrap();
    assert_eq!((c.url_origin, c.token_origin), (Origin::Env, Origin::Flag));
}
