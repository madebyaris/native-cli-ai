//! Agent Client Protocol (ACP) server for nca.
//!
//! Runs the existing nca runtime in-process and exposes it over ACP v1 on
//! stdio. No TUI, no Unix socket, no second binary.

mod approval;
mod history;
mod options;
mod server;
mod session;
mod translate;

pub use server::{AcpOptions, AcpServer};

use agent_client_protocol::schema::v1::{
    CancelNotification, DeleteSessionRequest, InitializeRequest, ListSessionsRequest,
    LoadSessionRequest, NewSessionRequest, PromptRequest, SetSessionConfigOptionRequest,
    SetSessionModeRequest,
};
use agent_client_protocol::{Agent, Stdio};
use nca_common::config::NcaConfig;
use std::sync::Arc;

/// Serve ACP on stdin/stdout until the client disconnects.
pub async fn run(config: NcaConfig, options: AcpOptions) -> anyhow::Result<()> {
    let server = Arc::new(AcpServer::new(config, options));

    let s1 = server.clone();
    let s2 = server.clone();
    let s3 = server.clone();
    let s4 = server.clone();
    let s5 = server.clone();
    let s6 = server.clone();
    let s7 = server.clone();
    let s8 = server.clone();
    let s9 = server.clone();

    Agent
        .builder()
        .name("nca")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                s1.initialize(request, responder).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, cx| {
                s2.new_session(request, responder, cx).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, cx| {
                s3.prompt(request, responder, cx).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, cx| {
                s4.load_session(request, responder, cx).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ListSessionsRequest, responder, _cx| {
                s5.list_sessions(request, responder).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionModeRequest, responder, cx| {
                s6.set_mode(request, responder, cx).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, cx| {
                s7.set_config_option(request, responder, cx).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeleteSessionRequest, responder, _cx| {
                s9.delete_session(request, responder).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, _cx| s8.cancel(notification).await,
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
        .map_err(|error| anyhow::anyhow!("ACP connection failed: {error}"))?;

    Ok(())
}
