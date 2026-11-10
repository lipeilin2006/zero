//! 内嵌终端会话：直接走 zero-proto 连服务器，不依赖外部终端模拟器。
//!
//! 每个标签页 = 一个会话：一条 iroh 连接 + 一条双向流 + 远端 PTY。
//! 输出用 base64 通过事件推给前端（前端可用 TextDecoder 流式解码，
//! 避免多字节 UTF-8 被切坏）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use iroh::{endpoint::presets, Endpoint, EndpointId, SecretKey};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Mutex};
use zero_proto::{read_msg, write_msg, ClientMsg, ServerMsg, ALPN};

/// 同时最多开多少个标签页，防止误点把内存打满。
const MAX_SESSIONS: usize = 8;

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// 远端 PTY 的输出（base64 原始字节）。
    Data { id: u32, data: String },
    /// 远端 shell 退出。
    Exit { id: u32, code: i32 },
    /// 连接或读取出错。
    Error { id: u32, message: String },
}

enum Outbound {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}

struct SessionHandle {
    outbound: mpsc::Sender<Outbound>,
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
}

pub struct SessionManager {
    app: AppHandle,
    endpoint: Mutex<Option<Endpoint>>,
    sessions: Mutex<HashMap<u32, SessionHandle>>,
    next_id: AtomicU32,
}

impl SessionManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            endpoint: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
        }
    }

    /// 复用 CLI 的客户端密钥（`$DATA/zero/secret.key`），
    /// 这样服务端已有的 authorized_keys 不用改。
    async fn endpoint(&self) -> Result<Endpoint> {
        let mut guard = self.endpoint.lock().await;
        if let Some(endpoint) = guard.as_ref() {
            return Ok(endpoint.clone());
        }
        let secret = load_or_create_secret(&key_path())?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret)
            .bind()
            .await
            .context("绑定 iroh 端点失败")?;
        endpoint.online().await;
        *guard = Some(endpoint.clone());
        Ok(endpoint)
    }

    pub async fn open(&self, endpoint_id: &str, cols: u16, rows: u16) -> Result<u32> {
        if self.sessions.lock().await.len() >= MAX_SESSIONS {
            bail!("最多同时打开 {MAX_SESSIONS} 个终端");
        }
        let server: EndpointId = endpoint_id
            .trim()
            .parse()
            .map_err(|_| anyhow!("Endpoint ID 格式不对"))?;

        let endpoint = self.endpoint().await?;
        let connection = endpoint
            .connect(server, ALPN)
            .await
            .context("连接服务器失败")?;
        let (mut send, mut recv) = connection.open_bi().await.context("打开双向流出错")?;

        // 握手：先要 shell，再等 Granted / Denied。
        write_msg(&mut send, &ClientMsg::Shell { cols, rows })
            .await
            .context("发送 Shell 请求失败")?;
        match read_msg::<_, ServerMsg>(&mut recv)
            .await
            .context("读取握手响应失败")?
        {
            Some(ServerMsg::Granted) => {}
            Some(ServerMsg::Denied { reason }) => bail!("服务器拒绝：{reason}"),
            Some(_) => bail!("服务器返回了意外的握手响应"),
            None => bail!("服务器在握手阶段断开连接"),
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (outbound, mut rx) = mpsc::channel::<Outbound>(128);

        let writer = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                let result = match message {
                    Outbound::Input(data) => write_msg(&mut send, &ClientMsg::Input { data }).await,
                    Outbound::Resize { cols, rows } => {
                        write_msg(&mut send, &ClientMsg::Resize { cols, rows }).await
                    }
                    Outbound::Close => {
                        let _ = write_msg(&mut send, &ClientMsg::Disconnect).await;
                        break;
                    }
                };
                if result.is_err() {
                    break;
                }
            }
            let _ = send.finish();
        });

        let app = self.app.clone();
        let conn = connection.clone();
        let reader = tokio::spawn(async move {
            loop {
                match read_msg::<_, ServerMsg>(&mut recv).await {
                    Ok(Some(ServerMsg::Output { data })) => {
                        let _ = app.emit(
                            "zero:session",
                            SessionEvent::Data {
                                id,
                                data: base64_encode(&data),
                            },
                        );
                    }
                    Ok(Some(ServerMsg::Exit { code })) => {
                        let _ = app.emit("zero:session", SessionEvent::Exit { id, code });
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        let _ = app.emit("zero:session", SessionEvent::Exit { id, code: -1 });
                        break;
                    }
                    Err(error) => {
                        let _ = app.emit(
                            "zero:session",
                            SessionEvent::Error {
                                id,
                                message: error.to_string(),
                            },
                        );
                        break;
                    }
                }
            }
            conn.close(0u8.into(), b"done");
        });

        self.sessions.lock().await.insert(
            id,
            SessionHandle {
                outbound,
                reader,
                writer,
            },
        );
        Ok(id)
    }

    pub async fn input(&self, id: u32, data: Vec<u8>) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let handle = sessions
            .get(&id)
            .ok_or_else(|| anyhow!("会话 {id} 不存在"))?;
        handle
            .outbound
            .send(Outbound::Input(data))
            .await
            .map_err(|_| anyhow!("会话 {id} 已关闭"))
    }

    pub async fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let Some(handle) = sessions.get(&id) else {
            return Ok(()); // 关掉的就别报错了
        };
        handle
            .outbound
            .send(Outbound::Resize {
                cols: cols.max(1),
                rows: rows.max(1),
            })
            .await
            .map_err(|_| anyhow!("会话 {id} 已关闭"))
    }

    pub async fn close(&self, id: u32) {
        if let Some(handle) = self.sessions.lock().await.remove(&id) {
            let _ = handle.outbound.send(Outbound::Close).await;
            // 给 Disconnect 一点时间发出去，再收掉读循环。
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            handle.reader.abort();
            handle.writer.abort();
        }
    }

    pub async fn close_all(&self) {
        let ids: Vec<u32> = self.sessions.lock().await.keys().copied().collect();
        for id in ids {
            self.close(id).await;
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn key_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zero")
        .join("secret.key")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        bail!("密钥不是合法的十六进制");
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|error| anyhow!(error))
        })
        .collect()
}

fn load_or_create_secret(path: &PathBuf) -> Result<SecretKey> {
    if path.exists() {
        let text = std::fs::read_to_string(path).context("读取客户端密钥失败")?;
        let text = text.trim();
        if let Ok(bytes) = hex_decode(text) {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("客户端密钥必须是 32 字节"))?;
            return Ok(SecretKey::from_bytes(&bytes));
        }
        return text.parse().context("解析客户端密钥失败");
    }

    let secret = SecretKey::generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("创建密钥目录失败")?;
    }
    std::fs::write(path, hex_encode(&secret.to_bytes())).context("写入客户端密钥失败")?;
    // 私钥别让同机其他用户读到。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(secret)
}
