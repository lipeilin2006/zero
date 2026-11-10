use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use iroh::{Endpoint, EndpointId, SecretKey, endpoint::presets};
use iroh_tickets::{Ticket, endpoint::EndpointTicket};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
use zero_proto::{ALPN, ClientMsg, ServerMsg};

#[derive(Parser, Debug)]
#[command(
    name = "zero-server",
    about = "iroh-based SSH-like shell server",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// 不带子命令时等价于 `zero-server run`
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 启动服务（不带子命令时默认走这个）
    Run(RunArgs),

    /// 信任一个客户端公钥（写进 authorized_keys）
    Trust {
        /// 客户端 endpoint id（zero --show-id 输出）
        client: String,
        /// 服务端私钥路径，用来推算默认的 authorized_keys 位置
        #[arg(long)]
        key: Option<PathBuf>,
        /// 直接指定 allowlist 文件
        #[arg(long)]
        allowlist: Option<PathBuf>,
    },

    /// 取消信任
    Untrust {
        client: String,
        #[arg(long)]
        key: Option<PathBuf>,
        #[arg(long)]
        allowlist: Option<PathBuf>,
    },

    /// 列出已信任的客户端
    Trusted {
        #[arg(long)]
        key: Option<PathBuf>,
        #[arg(long)]
        allowlist: Option<PathBuf>,
    },

    /// 打印服务器自己的 endpoint id
    Id {
        #[arg(long)]
        key: Option<PathBuf>,
    },

    /// 打印带地址的 ticket（推荐客户端用这个，不依赖发现服务）
    Ticket {
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

#[derive(Parser, Debug, Default)]
struct RunArgs {
    /// Path to secret key file (persistent identity)
    #[arg(long)]
    key: Option<PathBuf>,

    /// Allow any client (no allowlist). Default: only keys in allowlist file
    #[arg(long)]
    allow_all: bool,

    /// Path to allowlist file (one endpoint id per line)
    #[arg(long)]
    allowlist: Option<PathBuf>,

    /// Shell program to spawn (default: $SHELL or cmd.exe)
    #[arg(long)]
    shell: Option<String>,

    /// Locale for the PTY, e.g. C.UTF-8 / zh_CN.UTF-8
    /// (default: keep the server's UTF-8 locale, otherwise C.UTF-8)
    #[arg(long)]
    locale: Option<String>,
}

fn default_key_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zero-server")
        .join("secret.key")
}

fn default_allowlist_path(key_path: &Path) -> PathBuf {
    key_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("authorized_keys")
}

fn load_or_create_secret(path: &PathBuf) -> Result<SecretKey> {
    if path.exists() {
        let data = std::fs::read(path).context("read key")?;
        let s = String::from_utf8_lossy(&data);
        let s = s.trim();
        if let Ok(bytes) = hex_decode(s) {
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("key must be 32 bytes"))?;
            restrict_permissions(path);
            return Ok(SecretKey::from_bytes(&arr));
        }
        let key: SecretKey = s.parse().context("parse key")?;
        restrict_permissions(path);
        Ok(key)
    } else {
        let key = SecretKey::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = key.to_bytes();
        std::fs::write(path, hex_encode(&bytes))?;
        restrict_permissions(path);
        Ok(key)
    }
}

/// 私钥只允许当前用户读写（Windows 上先不做处理）。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

fn load_allowlist(path: &Path) -> Result<Vec<EndpointId>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    let mut ids = Vec::new();
    for (number, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let id: EndpointId = line
            .parse()
            .with_context(|| format!("第 {} 行不是合法的 endpoint id：{line}", number + 1))?;
        ids.push(id);
    }
    Ok(ids)
}

/// 原子写 allowlist：先写临时文件再替换。
fn write_allowlist(path: &Path, ids: &[EndpointId]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = String::from("# zero-server 信任的客户端，一行一个 endpoint id\n");
    for id in ids {
        text.push_str(&id.to_string());
        text.push('\n');
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 允许列表带 mtime 缓存：`zero-server trust` 改完文件不用重启。
struct Allowlist {
    path: PathBuf,
    cache: Mutex<(Option<SystemTime>, Vec<EndpointId>)>,
}

impl Allowlist {
    fn new(path: PathBuf) -> Self {
        let ids = load_allowlist(&path).unwrap_or_default();
        let mtime = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok();
        Self {
            path,
            cache: Mutex::new((mtime, ids)),
        }
    }

    /// 文件变了就读一遍；返回当前允许的列表。
    fn entries(&self) -> Vec<EndpointId> {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .ok();
        let mut cache = self.cache.lock().expect("allowlist cache poisoned");
        if cache.0 != mtime {
            match load_allowlist(&self.path) {
                Ok(ids) => {
                    info!(count = ids.len(), path = %self.path.display(), "allowlist reloaded");
                    cache.1 = ids;
                }
                Err(error) => warn!(%error, "allowlist reload failed, keeping previous"),
            }
            cache.0 = mtime;
        }
        cache.1.clone()
    }
}

/// 选一个 UTF-8 locale 给远端 PTY：
/// - 命令行指定优先；
/// - 否则沿用服务端自己的 UTF-8 locale；
/// - 都没有就用 C.UTF-8（glibc 系统一般都有）。
fn pick_locale(explicit: Option<&str>) -> String {
    if let Some(locale) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        return locale.to_string();
    }
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            let lower = value.to_lowercase();
            if lower.contains("utf-8") || lower.contains("utf8") {
                return value;
            }
        }
    }
    "C.UTF-8".to_string()
}

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

fn resolve_allowlist_path(key: Option<PathBuf>, allowlist: Option<PathBuf>) -> PathBuf {
    let key_path = key.unwrap_or_else(default_key_path);
    allowlist.unwrap_or_else(|| default_allowlist_path(&key_path))
}

fn parse_client(value: &str) -> Result<EndpointId> {
    value
        .trim()
        .parse()
        .with_context(|| format!("不是合法的 endpoint id：{value}"))
}

fn trust(key: Option<PathBuf>, allowlist: Option<PathBuf>, client: &str) -> Result<()> {
    let path = resolve_allowlist_path(key, allowlist);
    let id = parse_client(client)?;
    let mut ids = load_allowlist(&path)?;
    if ids.contains(&id) {
        println!("已经信任过：{id}");
        println!("allowlist：{}（共 {} 个）", path.display(), ids.len());
        return Ok(());
    }
    ids.push(id);
    write_allowlist(&path, &ids)?;
    println!("已信任：{id}");
    println!("allowlist：{}（共 {} 个）", path.display(), ids.len());
    println!("服务端在跑的话会自动生效，不用重启。");
    Ok(())
}

fn untrust(key: Option<PathBuf>, allowlist: Option<PathBuf>, client: &str) -> Result<()> {
    let path = resolve_allowlist_path(key, allowlist);
    let id = parse_client(client)?;
    let mut ids = load_allowlist(&path)?;
    let before = ids.len();
    ids.retain(|entry| entry != &id);
    if ids.len() == before {
        bail!("allowlist 里没有 {id}");
    }
    write_allowlist(&path, &ids)?;
    println!("已取消信任：{id}");
    println!("allowlist：{}（剩 {} 个）", path.display(), ids.len());
    Ok(())
}

fn trusted(key: Option<PathBuf>, allowlist: Option<PathBuf>) -> Result<()> {
    let path = resolve_allowlist_path(key, allowlist);
    let ids = load_allowlist(&path)?;
    if ids.is_empty() {
        println!("还没有信任任何客户端：{}", path.display());
        println!("用 zero --show-id 拿到客户端 id，再执行 zero-server trust <id>");
        return Ok(());
    }
    println!("{} 个受信任的客户端（{}）：", ids.len(), path.display());
    for (index, id) in ids.iter().enumerate() {
        println!("{:>3}. {}", index + 1, id);
    }
    Ok(())
}

/// Cross-platform shutdown: Ctrl+C everywhere, SIGTERM on Unix (systemd etc).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Trust {
            client,
            key,
            allowlist,
        }) => trust(key, allowlist, &client),
        Some(Command::Untrust {
            client,
            key,
            allowlist,
        }) => untrust(key, allowlist, &client),
        Some(Command::Trusted { key, allowlist }) => trusted(key, allowlist),
        Some(Command::Id { key }) => {
            let path = key.unwrap_or_else(default_key_path);
            let secret = load_or_create_secret(&path)?;
            println!("{}", secret.public());
            println!("key file: {}", path.display());
            Ok(())
        }
        Some(Command::Ticket { key }) => print_ticket(key).await,
        Some(Command::Run(args)) => run(args).await,
        None => run(cli.run).await,
    }
}

/// 起一个临时 endpoint 拿到带地址的 ticket 后退出。
async fn print_ticket(key: Option<PathBuf>) -> Result<()> {
    let path = key.unwrap_or_else(default_key_path);
    let secret = load_or_create_secret(&path)?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await?;
    endpoint.online().await;
    println!("{}", EndpointTicket::new(endpoint.addr()).encode_string());
    endpoint.close().await;
    Ok(())
}

async fn run(args: RunArgs) -> Result<()> {
    let key_path = args.key.clone().unwrap_or_else(default_key_path);
    let secret = load_or_create_secret(&key_path)?;
    let endpoint_id = secret.public();
    info!("platform: {}", std::env::consts::OS);
    info!("server endpoint id: {}", endpoint_id);
    info!("key file: {}", key_path.display());

    let allowlist = if args.allow_all {
        info!("allowing all clients (--allow-all)");
        None
    } else {
        let path = args
            .allowlist
            .clone()
            .unwrap_or_else(|| default_allowlist_path(&key_path));
        let watch = Arc::new(Allowlist::new(path.clone()));
        let count = watch.entries().len();
        if count == 0 {
            warn!(
                "allowlist empty, denying all clients. Add keys with `zero-server trust <id>` ({}), or use --allow-all",
                path.display()
            );
        } else {
            info!("loaded {count} allowed client(s) from {}", path.display());
        }
        Some(watch)
    };

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;

    endpoint.online().await;
    let addr = endpoint.addr();
    info!("listening. EndpointAddr: {addr:?}");
    let ticket = EndpointTicket::new(addr).encode_string();
    info!("server ticket: {ticket}");
    info!("clients can connect with: zero {ticket}");

    let shell = args.shell.clone().unwrap_or_else(default_shell);
    let locale = pick_locale(args.locale.as_deref());
    info!("shell: {shell}");
    info!("pty locale: {locale}");

    let sessions = Arc::new(Semaphore::new(32));
    let endpoint_for_shutdown = endpoint.clone();
    tokio::select! {
        _ = shutdown_signal() => {
            info!("shutdown signal received");
        }
        _ = accept_loop(&endpoint, allowlist, shell, locale, sessions) => {
            info!("endpoint closed");
        }
    }
    endpoint_for_shutdown.close().await;
    info!("server stopped");
    Ok(())
}

async fn accept_loop(
    endpoint: &Endpoint,
    allowlist: Option<Arc<Allowlist>>,
    shell: String,
    locale: String,
    sessions: Arc<Semaphore>,
) {
    while let Some(incoming) = endpoint.accept().await {
        // 最多同时 32 个会话，超出直接拒，防止被连接打满。
        let Ok(permit) = sessions.clone().try_acquire_owned() else {
            warn!("session limit reached, rejecting connection");
            continue;
        };
        let allowlist = allowlist.clone();
        let shell = shell.clone();
        let locale = locale.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_incoming(incoming, allowlist, shell, locale).await {
                error!("connection error: {e:#}");
            }
        });
    }
}

/// 拒绝时把原因发出去再关连接；直接 drop 客户端会只看到 connection closed。
async fn deny(send: &mut iroh::endpoint::SendStream, reason: &str) {
    let _ = zero_proto::write_msg(
        send,
        &ServerMsg::Denied {
            reason: reason.to_string(),
        },
    )
    .await;
    let _ = send.finish();
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
}

async fn handle_incoming(
    incoming: iroh::endpoint::Incoming,
    allowlist: Option<Arc<Allowlist>>,
    shell: String,
    locale: String,
) -> Result<()> {
    let accepting = incoming.accept()?;
    let conn = accepting.await?;
    let peer = conn.remote_id();
    info!("incoming connection from {peer}");

    let (mut send, mut recv) = conn.accept_bi().await?;

    let msg = zero_proto::read_msg::<_, ClientMsg>(&mut recv)
        .await?
        .context("expected first message")?;
    let ClientMsg::Shell { cols, rows } = msg else {
        deny(&mut send, "expected Shell request").await;
        return Ok(());
    };

    let denied = allowlist
        .as_ref()
        .is_some_and(|watch| !watch.entries().contains(&peer));
    if denied {
        info!("denied {peer} (not in allowlist)");
        deny(&mut send, "not authorized").await;
        return Ok(());
    }

    zero_proto::write_msg(&mut send, &ServerMsg::Granted).await?;
    info!("granted shell to {peer} ({cols}x{rows})");

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(&shell);
    cmd.env(
        "TERM",
        std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
    );
    cmd.env("ZERO_SSH", "1");
    // 强制 UTF-8：中文、emoji、宽字符不依赖服务端启动环境。
    cmd.env("LANG", &locale);
    cmd.env("LC_ALL", &locale);
    cmd.env("LC_CTYPE", &locale);

    let child = pair.slave.spawn_command(cmd)?;
    info!("pty shell spawned for {peer}");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let master = pair.master;

    let (tx_out, mut rx_out) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (tx_in, mut rx_in) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    let out_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx_out.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!("pty reader stopped: {error}");
                    break;
                }
            }
        }
    });

    let in_handle = tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        while let Some(data) = rx_in.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    let mut child = child;
    let mut exit_handle = tokio::task::spawn_blocking(move || {
        child
            .wait()
            .ok()
            .map(|s| s.exit_code() as i32)
            .unwrap_or(-1)
    });

    let mut exit_code: Option<i32> = None;
    loop {
        tokio::select! {
            biased;

            data = rx_out.recv() => {
                match data {
                    Some(data) => {
                        zero_proto::write_msg(&mut send, &ServerMsg::Output { data }).await?;
                    }
                    None => break,
                }
            }

            msg = zero_proto::read_msg::<_, ClientMsg>(&mut recv) => {
                match msg {
                    Ok(Some(ClientMsg::Input { data })) => {
                        if tx_in.send(data).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(ClientMsg::Resize { cols, rows })) => {
                        let _ = master.resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                    Ok(Some(ClientMsg::Shell { .. })) => {
                        // already handled before session start
                    }
                    Ok(Some(ClientMsg::Disconnect)) | Ok(None) => break,
                    Err(e) => {
                        error!("read error: {e}");
                        break;
                    }
                }
            }

            code = &mut exit_handle => {
                exit_code = code.ok();
                break;
            }
        }
    }

    drop(tx_in);
    let _ = in_handle.await;
    drop(rx_out);
    let _ = out_handle.await;

    let code = exit_code.unwrap_or(-1);
    info!("session with {peer} exited code={code}");
    let _ = zero_proto::write_msg(&mut send, &ServerMsg::Exit { code }).await;

    conn.close(0u8.into(), b"done");
    Ok(())
}
