//! 内嵌终端会话：直接走 zero-proto 连服务器，不依赖外部终端模拟器。
//!
//! 每个标签页 = 一个会话：一条 iroh 连接 + 一条双向流 + 远端 PTY。
//! 输出用 base64 通过事件推给前端（前端可用 TextDecoder 流式解码，
//! 避免多字节 UTF-8 被切坏）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use iroh::{endpoint::presets, Endpoint, EndpointAddr, SecretKey};
use iroh_tickets::{endpoint::EndpointTicket, Ticket};
use serde::Serialize;
use std::time::Instant;
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{timeout, Duration};
use zero_proto::{read_msg, write_msg, ClientMsg, ServerMsg, ALPN};

/// 同时最多开多少个标签页，防止误点把内存打满。
const MAX_SESSIONS: usize = 8;

#[derive(Clone, Debug, Serialize)]
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

/// 事件出口：生产环境是 Tauri 事件，测试里可以换成收集器。
pub trait SessionEmitter: Send + Sync + 'static {
    fn emit(&self, event: SessionEvent);
}

struct TauriEmitter(AppHandle);

impl SessionEmitter for TauriEmitter {
    fn emit(&self, event: SessionEvent) {
        use tauri::Emitter as _;
        let _ = self.0.emit("zero:session", event);
    }
}

pub struct SessionManager {
    emitter: Arc<dyn SessionEmitter>,
    endpoint: Mutex<Option<Endpoint>>,
    sessions: Mutex<HashMap<u32, SessionHandle>>,
    next_id: AtomicU32,
}

impl SessionManager {
    pub fn new(app: AppHandle) -> Self {
        Self::with_emitter(Arc::new(TauriEmitter(app)))
    }

    pub fn with_emitter(emitter: Arc<dyn SessionEmitter>) -> Self {
        Self {
            emitter,
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
        let endpoint = timeout(Duration::from_secs(20), async {
            Endpoint::builder(presets::N0)
                .secret_key(secret)
                .bind()
                .await
        })
        .await
        .map_err(|_| anyhow!("初始化网络超时（检查网络或防火墙是否拦了 UDP）"))?
        .context("绑定 iroh 端点失败")?;
        // 只等一小会儿 relay：ticket 自带地址，裸 id 也能在 connect 里再等。
        let _ = timeout(Duration::from_secs(2), endpoint.online()).await;
        *guard = Some(endpoint.clone());
        Ok(endpoint)
    }

    pub async fn open(&self, target: &str, cols: u16, rows: u16) -> Result<u32> {
        if self.sessions.lock().await.len() >= MAX_SESSIONS {
            bail!("最多同时打开 {MAX_SESSIONS} 个终端");
        }
        let addr = parse_addr(target)?;

        let endpoint = self.endpoint().await?;

        let connection = connect_with_retry(&endpoint, addr).await?;
        let (mut send, mut recv) = timeout(Duration::from_secs(15), connection.open_bi())
            .await
            .map_err(|_| anyhow!("打开流超时"))?
            .context("打开双向流出错")?;

        // 握手：先要 shell，再等 Granted / Denied。
        timeout(
            Duration::from_secs(10),
            write_msg(&mut send, &ClientMsg::Shell { cols, rows }),
        )
        .await
        .map_err(|_| anyhow!("发送 Shell 请求超时"))?
        .context("发送 Shell 请求失败")?;
        let handshake = timeout(Duration::from_secs(15), read_msg::<_, ServerMsg>(&mut recv))
            .await
            .map_err(|_| anyhow!("服务器长时间没有响应（确认服务端版本和防火墙）"))?
            .context("读取握手响应失败")?;
        match handshake {
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

        let emitter = self.emitter.clone();
        let conn = connection.clone();
        let reader = tokio::spawn(async move {
            loop {
                match read_msg::<_, ServerMsg>(&mut recv).await {
                    Ok(Some(ServerMsg::Output { data })) => {
                        emitter.emit(SessionEvent::Data {
                            id,
                            data: base64_encode(&data),
                        });
                    }
                    Ok(Some(ServerMsg::Exit { code })) => {
                        emitter.emit(SessionEvent::Exit { id, code });
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        emitter.emit(SessionEvent::Exit { id, code: -1 });
                        break;
                    }
                    Err(error) => {
                        let message = error.to_string();
                        // 正常收尾时服务器会直接关连接，不必当错误报。
                        if message.contains("connection lost") || message.contains("closed by peer")
                        {
                            emitter.emit(SessionEvent::Exit { id, code: -1 });
                        } else {
                            emitter.emit(SessionEvent::Error { id, message });
                        }
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

/// 连接带重试：裸 endpoint id 要靠发现服务拿地址，服务端刚启动时
/// 可能还查不到（"No addressing information"）。ticket 自带地址，基本一次就成。
async fn connect_with_retry(
    endpoint: &Endpoint,
    addr: EndpointAddr,
) -> Result<iroh::endpoint::Connection> {
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        match timeout(
            Duration::from_secs(15),
            endpoint.connect(addr.clone(), ALPN),
        )
        .await
        {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => {
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "连接失败：{error}\n如果服务器刚启动，等几秒再试；或直接服务端日志里的 ticket（带地址）会更稳。"
                    ));
                }
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "连接超时：服务器不可达或地址没有解析出来。\n可以换成服务端日志里的 ticket（带地址）再试。"
                    ));
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// 支持两种写法：`zero://...` 这种 ticket（带地址），或裸 endpoint id。
fn parse_addr(value: &str) -> Result<EndpointAddr> {
    let value = value.trim();
    if value.is_empty() {
        bail!("endpoint 不能为空");
    }
    if let Ok(ticket) = EndpointTicket::decode_string(value) {
        return Ok(ticket.endpoint_addr().clone());
    }
    let id = value
        .parse()
        .map_err(|_| anyhow!("既不是 endpoint id，也不是合法的 ticket"))?;
    Ok(EndpointAddr::new(id))
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
