//! Shell completion generation and installation.

use crate::cli::{ClapShell, Cli};
use clap::CommandFactory;
use clap_complete::aot::generate;
use std::path::PathBuf;

pub fn install_shell_completion(
    shell: Option<ClapShell>,
    path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let shell = shell.unwrap_or_else(detect_shell);
    let home = std::env::var("HOME").unwrap_or_default();
    let target = match path {
        Some(p) => p,
        None => match shell {
            ClapShell::Bash => PathBuf::from(format!(
                "{home}/.local/share/bash-completion/completions/nca"
            )),
            ClapShell::Zsh => PathBuf::from(format!("{home}/.zsh/completions/_nca")),
            ClapShell::Fish => PathBuf::from(format!("{home}/.config/fish/completions/nca.fish")),
            ClapShell::PowerShell | ClapShell::Elvish => {
                anyhow::bail!(
                    "auto-install is not supported for this shell; run `nca completion generate {shell:?}` and redirect to your preferred location"
                );
            }
        },
    };

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut cmd = Cli::command();
    let bin_name = "nca";
    let mut buf: Vec<u8> = Vec::new();
    match shell {
        ClapShell::Bash => generate(clap_complete::shells::Bash, &mut cmd, bin_name, &mut buf),
        ClapShell::Zsh => generate(clap_complete::shells::Zsh, &mut cmd, bin_name, &mut buf),
        ClapShell::Fish => generate(clap_complete::shells::Fish, &mut cmd, bin_name, &mut buf),
        ClapShell::PowerShell => generate(
            clap_complete::shells::PowerShell,
            &mut cmd,
            bin_name,
            &mut buf,
        ),
        ClapShell::Elvish => generate(clap_complete::shells::Elvish, &mut cmd, bin_name, &mut buf),
    }
    std::fs::write(&target, &buf)?;
    println!("✓ installed {shell:?} completions to {}", target.display());

    if matches!(shell, ClapShell::Zsh) {
        println!(
            "  hint: ensure `fpath=(~/.zsh/completions $fpath)` and `autoload -U compinit && compinit` are in your ~/.zshrc"
        );
    }
    Ok(())
}

pub fn detect_shell() -> ClapShell {
    let sh = std::env::var("SHELL").unwrap_or_default();
    if sh.ends_with("zsh") {
        ClapShell::Zsh
    } else if sh.ends_with("fish") {
        ClapShell::Fish
    } else {
        ClapShell::Bash
    }
}

pub fn generate_shell_completion(shell: ClapShell) {
    let mut cmd = Cli::command();
    let bin_name = "nca";

    match shell {
        ClapShell::Bash => {
            generate(
                clap_complete::shells::Bash,
                &mut cmd,
                bin_name,
                &mut std::io::stdout(),
            );
        }
        ClapShell::Zsh => {
            generate(
                clap_complete::shells::Zsh,
                &mut cmd,
                bin_name,
                &mut std::io::stdout(),
            );
        }
        ClapShell::Fish => {
            generate(
                clap_complete::shells::Fish,
                &mut cmd,
                bin_name,
                &mut std::io::stdout(),
            );
        }
        ClapShell::PowerShell => {
            generate(
                clap_complete::shells::PowerShell,
                &mut cmd,
                bin_name,
                &mut std::io::stdout(),
            );
        }
        ClapShell::Elvish => {
            generate(
                clap_complete::shells::Elvish,
                &mut cmd,
                bin_name,
                &mut std::io::stdout(),
            );
        }
    }
}
