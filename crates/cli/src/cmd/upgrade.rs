//! Local binary upgrade via cargo build.

use std::path::PathBuf;

pub fn run_upgrade(install_dir: Option<PathBuf>, no_test: bool) -> anyhow::Result<()> {
    use std::process::Command;

    let install_dir = install_dir.unwrap_or_else(|| PathBuf::from("/usr/local/bin"));
    let repo_root = find_repo_root().ok_or_else(|| {
        anyhow::anyhow!(
            "could not locate workspace Cargo.toml; run `nca upgrade` from within the nca source tree"
        )
    })?;

    println!("→ building release binary in {}", repo_root.display());
    let mut build = Command::new("cargo");
    build
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg("nca-cli")
        .current_dir(&repo_root);
    let status = build.status()?;
    if !status.success() {
        anyhow::bail!("cargo build failed (exit code {:?})", status.code());
    }

    if !no_test {
        println!("→ running workspace tests");
        let status = Command::new("cargo")
            .arg("test")
            .arg("--workspace")
            .arg("--lib")
            .current_dir(&repo_root)
            .status()?;
        if !status.success() {
            anyhow::bail!(
                "tests failed (exit code {:?}); pass --no-test to skip",
                status.code()
            );
        }
    }

    let binary = repo_root.join("target/release/nca");
    if !binary.exists() {
        anyhow::bail!("built binary not found at {}", binary.display());
    }
    let target = install_dir.join("nca");
    std::fs::create_dir_all(&install_dir)?;
    std::fs::copy(&binary, &target)?;
    println!("✓ installed {} → {}", binary.display(), target.display());
    Ok(())
}

pub fn find_repo_root() -> Option<PathBuf> {
    let mut cur = std::env::current_dir().ok()?;
    loop {
        if cur.join("Cargo.toml").exists()
            && std::fs::read_to_string(cur.join("Cargo.toml"))
                .map(|s| s.contains("[workspace]"))
                .unwrap_or(false)
        {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}
