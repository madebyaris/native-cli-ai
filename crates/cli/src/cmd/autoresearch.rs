//! Autonomous research metric runner.

use std::path::PathBuf;

pub async fn autoresearch_once(program: PathBuf, workspace: PathBuf) -> anyhow::Result<()> {
    use nca_autoresearch::experiment::{ExperimentConfig, ExperimentRunner};
    use nca_autoresearch::metric_parser::MetricParser;
    use nca_autoresearch::program::ResearchProgram;

    let prog = ResearchProgram::from_file(&program).map_err(|e| anyhow::anyhow!("{e}"))?;
    let shell_cmd = prog
        .metric_command
        .command
        .trim()
        .trim_matches('`')
        .trim()
        .replace('\n', " ");
    if shell_cmd.is_empty() {
        anyhow::bail!("research program has an empty metric cmd/command");
    }

    println!("workspace: {}", workspace.display());
    println!("program:   {}", program.display());
    println!("metric:    regex {:?}", prog.metric_command.parse_regex);
    println!("running:   sh -c {}", shell_cmd);

    let cfg = ExperimentConfig {
        working_dir: workspace,
        command: "sh".into(),
        args: vec!["-c".into(), shell_cmd.to_string()],
        time_budget_seconds: prog.time_budget_seconds,
        log_file: None,
        memory_limit_gb: prog.max_memory_gb,
        kill_timeout_factor: 2,
    };
    let runner = ExperimentRunner::new(cfg);
    let output = runner
        .run_with_description("nca autoresearch once".into())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let parser = MetricParser::new();
    let metric = parser
        .extract_with_regex(&output.output, &prog.metric_command.parse_regex)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "could not parse metric with regex {:?} from output (first 800 chars):\n{}",
                prog.metric_command.parse_regex,
                output.output.chars().take(800).collect::<String>()
            )
        })?;

    println!("\n---");
    println!("metric value: {metric}");
    println!("experiment status: {:?}", output.status);
    println!("---");
    Ok(())
}
