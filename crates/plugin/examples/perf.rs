//! Hook latency benchmark: `cargo run --release -p nca-plugin --example perf -- [spin]`.
//!
//! Uses whatever plugins `NCA_PLUGIN_HOME` (or `~/.nca/plugins`) has installed.
//! With `spin`, also times the sandbox timeout / memory cap. That needs the
//! `examples/plugin-spin` fixture installed first: `nca add examples/plugin-spin --yes`
//! (use a throwaway `NCA_PLUGIN_HOME`; it is a test fixture, not a plugin to keep).

use nca_common::config::{NcaConfig, ProviderKind};
use nca_plugin::{SpawnRequest, ThemeEntries};
use std::hint::black_box;
use std::time::{Duration, Instant};

const WARMUP: usize = 200;
const ITERS: usize = 3000;

/// Times only `f`. `setup` builds each call's owned input outside the timed region, so the
/// numbers are hook latency, not the cost of cloning arguments.
fn bench<T>(name: &str, mut setup: impl FnMut() -> T, mut f: impl FnMut(T)) {
    for _ in 0..WARMUP {
        f(setup());
    }
    let mut samples: Vec<Duration> = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let input = setup();
        let t = Instant::now();
        f(input);
        samples.push(t.elapsed());
    }
    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let mean = samples.iter().map(|d| us(*d)).sum::<f64>() / ITERS as f64;
    println!(
        "{name:<26} mean {mean:>9.2} us   p50 {:>9.2} us   p99 {:>9.2} us",
        us(samples[ITERS / 2]),
        us(samples[ITERS * 99 / 100])
    );
}

fn main() {
    let spin = std::env::args().any(|a| a == "spin");
    let ws = std::env::current_dir().unwrap();

    let t = Instant::now();
    nca_plugin::init(&ws);
    let load = t.elapsed();
    println!(
        "plugins loaded: {:?}   init (verify+compile+instantiate): {:.1} ms",
        nca_plugin::loaded_names(),
        load.as_secs_f64() * 1e3
    );

    let mut config = NcaConfig::default();
    config.provider.default = ProviderKind::Anthropic;
    let prompt = "You are nca. ".repeat(600); // ~8 KB
    let theme: ThemeEntries = [
        "bg",
        "surface",
        "border",
        "mention-bg",
        "user",
        "assistant",
        "tool",
        "muted",
        "text",
        "success",
        "error",
        "warn",
    ]
    .iter()
    .map(|t| (t.to_string(), "#202020".to_string()))
    .collect();
    let req = SpawnRequest {
        task: "refactor module".into(),
        focus_files: vec!["src/lib.rs".into()],
        skills: vec![],
        use_worktree: false,
    };

    bench(
        "auth.resolve-credential",
        || config.clone(),
        |mut c| {
            black_box(nca_plugin::apply_auth(&mut c).ok());
        },
    );
    bench(
        "subagent.before-spawn",
        || req.clone(),
        |r| {
            black_box(nca_plugin::before_spawn(r));
        },
    );
    bench(
        "agent.system-prompt (8KB)",
        || prompt.clone(),
        |p| {
            black_box(nca_plugin::transform_system_prompt(p));
        },
    );
    bench(
        "agent.before-tool-use",
        || (),
        |()| {
            black_box(nca_plugin::before_tool_use("bash", r#"{"cmd":"ls -la"}"#));
        },
    );
    bench(
        "cosmetic.theme",
        || theme.clone(),
        |t| {
            black_box(nca_plugin::theme(t));
        },
    );

    if spin {
        for tool in ["loop", "hog"] {
            let t = Instant::now();
            let d = nca_plugin::before_tool_use(tool, "{}");
            println!(
                "sandbox `{tool}`: returned after {:.2} s -> {d:?}",
                t.elapsed().as_secs_f64()
            );
        }
        let t = Instant::now();
        let d = nca_plugin::before_tool_use("bash", "{}");
        println!(
            "call after trap: {:.2} s -> {d:?}",
            t.elapsed().as_secs_f64()
        );
    }
}
