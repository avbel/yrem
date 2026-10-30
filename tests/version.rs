//! `yrem --version` comes from Cargo.toml.

#[test]
fn version_flag_prints_cargo_version() {
    for flag in ["--version", "-V"] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_yrem"))
            .arg(flag)
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            concat!("yrem ", env!("CARGO_PKG_VERSION"))
        );
    }
}
