use std::thread;

use tiny_http::{Header, Request, Response, Server, StatusCode};

use super::StreamChunk;

pub fn spawn_sse_server<F>(body: String, status: u16, assert_request: F) -> String
where
    F: FnOnce(&Request) + Send + 'static,
{
    let server = Server::http("127.0.0.1:0").expect("start mock server");
    let base_url = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => format!("http://{addr}"),
        other => panic!("unsupported listen addr: {other:?}"),
    };

    thread::spawn(move || {
        let request = server.recv().expect("receive request");
        assert_request(&request);
        let response = Response::from_string(body)
            .with_status_code(StatusCode(status))
            .with_header(
                Header::from_bytes("Content-Type", "text/event-stream")
                    .expect("content type header"),
            );
        request.respond(response).expect("send response");
    });

    base_url
}

pub async fn collect_chunks(mut rx: tokio::sync::mpsc::Receiver<StreamChunk>) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        let done = matches!(chunk, StreamChunk::Done);
        chunks.push(chunk);
        if done {
            break;
        }
    }
    chunks
}
