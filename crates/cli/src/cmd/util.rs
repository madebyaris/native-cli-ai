//! Shared CLI output helpers.

use crate::stream;
use nca_common::event::EventEnvelope;
use nca_common::session::SessionSnapshot;

pub fn print_json<T: serde::Serialize>(value: &T, pretty: bool) -> anyhow::Result<()> {
    let rendered = if pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    println!("{rendered}");
    Ok(())
}

pub fn print_human_session(session: &SessionSnapshot) {
    println!(
        "{}  status={:?}  model={}  updated={}  children={}",
        session.id,
        session.status,
        session.model,
        session.updated_at.to_rfc3339(),
        session.child_session_ids.len()
    );
    if let Some(summary) = &session.session_summary {
        println!("  summary: {}", summary.replace('\n', " "));
    }
}

pub fn print_event_envelope(envelope: &EventEnvelope, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(envelope)?);
    } else {
        stream::render_human_event(&envelope.event);
    }
    Ok(())
}
