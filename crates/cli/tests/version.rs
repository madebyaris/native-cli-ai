use assert_cmd::Command;

#[test]
fn version_flags_print_the_workspace_version() {
    let expected = format!("nca {}", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let output = Command::cargo_bin("nca")
            .expect("binary")
            .arg(flag)
            .output()
            .expect("run nca");
        assert!(output.status.success(), "{flag} should exit zero");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), expected, "{flag} output");
    }
}
