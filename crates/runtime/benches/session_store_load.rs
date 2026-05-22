//! Criterion benches for SessionStore (Phase 1.6).
//!
//! Run with `cargo bench -p nca-runtime`. These track regressions against
//! baselines recorded in docs/research/baselines.md.

#![allow(clippy::pedantic, dead_code, unused_imports, unused_mut)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nca_common::event::AgentEvent;
use nca_common::message::Message;
use nca_common::session::{SESSION_STATE_SCHEMA_VERSION, SessionMeta, SessionState, SessionStatus};
use nca_runtime::context_manager::ContextManager;
use nca_runtime::session_store::SessionStore;
use std::hint::black_box;
use tempfile::TempDir;
use tokio::runtime::Runtime;

fn make_session(id: &str, n_messages: usize) -> SessionState {
    let now = chrono::Utc::now();
    let meta = SessionMeta {
        id: id.into(),
        created_at: now,
        updated_at: now,
        workspace: std::path::PathBuf::from("/tmp/bench"),
        model: "MiniMax-M2".into(),
        status: SessionStatus::Running,
        pid: None,
        socket_path: None,
        worktree_path: None,
        branch: None,
        base_branch: None,
        parent_session_id: None,
        child_session_ids: Vec::new(),
        inherited_summary: None,
        spawn_reason: None,
        session_summary: None,
        orchestration: None,
    };
    let mut messages = Vec::with_capacity(n_messages * 2);
    for i in 0..n_messages {
        messages.push(Message::user(format!("bench message {i}")));
        messages.push(Message::assistant(format!("reply {i}")));
    }
    SessionState {
        schema_version: SESSION_STATE_SCHEMA_VERSION,
        meta,
        messages,
        total_input_tokens: 0,
        total_output_tokens: 0,
        estimated_cost_usd: 0.0,
    }
}

fn bench_session_store_load(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let tempdir = TempDir::new().unwrap();
    let store = SessionStore::new(tempdir.path());

    let mut group = c.benchmark_group("session_store_load");
    for size in [10usize, 100, 500] {
        let id = format!("bench-{size}");
        let state = make_session(&id, size);
        rt.block_on(store.save(&state)).unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(size), &id, |b, id| {
            b.iter(|| {
                let loaded = rt.block_on(store.load(black_box(id))).unwrap();
                black_box(loaded.messages.len());
            });
        });
    }
    group.finish();
}

fn bench_event_serialize(c: &mut Criterion) {
    let event = AgentEvent::TokensStreamed {
        delta: "hello world".repeat(10),
    };
    c.bench_function("event_serialize", |b| {
        b.iter(|| {
            let s = serde_json::to_string(black_box(&event)).unwrap();
            black_box(s);
        });
    });
}

fn bench_context_manager(c: &mut Criterion) {
    let mut group = c.benchmark_group("context_manager");
    let mgr = ContextManager::with_default_config("MiniMax-M2".into());

    for size in [50usize, 200, 1000] {
        let mut msgs = Vec::with_capacity(size);
        for i in 0..size {
            if i % 2 == 0 {
                msgs.push(Message::user(format!(
                    "user prompt {i} with some medium length content to exercise the estimator"
                )));
            } else {
                msgs.push(Message::assistant(format!(
                    "assistant reply {i} containing analysis, code snippets, and follow up ideas"
                )));
            }
        }

        group.bench_with_input(
            BenchmarkId::new("estimate_tokens", size),
            &msgs,
            |b, msgs| {
                b.iter(|| black_box(ContextManager::estimate_tokens_for_slice(black_box(msgs))));
            },
        );

        group.bench_with_input(BenchmarkId::new("stats", size), &msgs, |b, msgs| {
            b.iter(|| black_box(mgr.stats(black_box(msgs))));
        });

        group.bench_with_input(
            BenchmarkId::new("get_compaction_plan", size),
            &msgs,
            |b, msgs| {
                b.iter(|| black_box(mgr.get_compaction_plan(black_box(msgs))));
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_session_store_load,
    bench_event_serialize,
    bench_context_manager,
);
criterion_main!(benches);
