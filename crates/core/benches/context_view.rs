use criterion::{Criterion, criterion_group, criterion_main};
use nca_common::config::SmartCompactionMode;
use nca_common::message::{Message, MessageToolCall};
use nca_core::context_view::{estimate_tokens_for_slice, plan_context_view};
use serde_json::json;

fn long_session() -> Vec<Message> {
    let mut messages = vec![Message::system("You are nca.")];
    for i in 0..60 {
        messages.push(Message::user(format!("batch {i}")));
        let output = format!("body {i}\n{}", "x".repeat(1_500));
        messages.push(Message::assistant_with_tool_calls(
            "",
            vec![MessageToolCall {
                id: format!("c{i}"),
                name: "read_file".into(),
                arguments: json!({"path": format!("f{i}.rs")}),
            }],
        ));
        messages.push(Message::tool(format!("c{i}"), output));
        messages.push(Message::assistant(format!("ok {i}")));
    }
    messages
}

fn bench_plan(c: &mut Criterion) {
    let messages = long_session();
    c.bench_function("context_view_plan_on", |b| {
        b.iter(|| plan_context_view(&messages, SmartCompactionMode::On))
    });
    let plan = plan_context_view(&messages, SmartCompactionMode::On);
    let before = estimate_tokens_for_slice(&messages);
    assert!(plan.report.tokens_after < before);
}

criterion_group!(benches, bench_plan);
criterion_main!(benches);
