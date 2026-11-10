//! 冒烟测试：不经过界面，直接验证 session 层能连上、发命令、收输出。
//!
//! 用法：cargo run -p zero-client --example session_smoke -- <server-endpoint-id>

use std::sync::Arc;
use std::time::Duration;

use zero_client_lib::session::{SessionEmitter, SessionEvent, SessionManager};

struct Printer;

impl SessionEmitter for Printer {
    fn emit(&self, event: SessionEvent) {
        match &event {
            SessionEvent::Data { data, .. } => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap_or_default();
                print!("{}", String::from_utf8_lossy(&bytes));
            }
            other => println!("\n[event] {other:?}"),
        }
    }
}

#[tokio::main]
async fn main() {
    let endpoint = std::env::args()
        .nth(1)
        .expect("usage: session_smoke <server-endpoint-id>");
    let manager = SessionManager::with_emitter(Arc::new(Printer));

    let id = match manager.open(&endpoint, 80, 24).await {
        Ok(id) => id,
        Err(error) => {
            eprintln!("open failed: {error:#}");
            std::process::exit(1);
        }
    };
    println!("\n[session {id} opened]");

    manager
        .input(id, b"printf 'SMOKE_OK\\n'\r".to_vec())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    manager.input(id, b"exit\r".to_vec()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    manager.close(id).await;
}
