use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use iroh::{Endpoint, EndpointId, SecretKey, endpoint::presets};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tracing::{error, info, warn};
use zero_proto::{ALPN, ClientMsg, ServerMsg};

#[derive(Parser, Debug)]
#[command(name = "zero-server", about = "iroh-based SSH-like shell server")]
struct Args {
    /// Path to secret key file (persistent identity)
    #[arg(long)]
    key: Option<PathBuf>,

    /// Allow any client (no allowlist). Default: only keys in allowlist file
    #[arg(long)]
    allow_all: bool,

    /// Path to allowlist file (one base32 public key per line)
    #[arg(long)]
    allowlist: Option<PathBuf>,

    /// Shell program to spawn (default: $SHELL or cmd.exe)
    #[arg(long)]
    shell: Option<String>,
}

fn default_key_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zero-server")
        .join("secret.key")
}

fn load_or_create_secret(path: &PathBuf) -> Result<SecretKey> {
    if path.exists() {
        let data = std::fs::read(path).context("read key")?;
        let s = String::from_utf8_lossy(&data);
        let s = s.trim();
        // Try hex first, then base32 via EndpointId-style parse
        if let Ok(bytes) = hex_decode(s) {
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("key must be 32 bytes"))?;
            return Ok(SecretKey::from_bytes(&arr));
        }
        // Fallback: parse as SecretKey string (serde)
        let key: SecretKey = s.parse().context("parse key")?;
        Ok(key)
    } else {
        let key = SecretKey::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = key.to_bytes();
        std::fs::write(path, hex_encode(&bytes))?;
        Ok(key)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

fn load_allowlist(path: &PathBuf) -> Result<Vec<EndpointId>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    let mut ids = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let id: EndpointId = line.parse().context("parse allowlist entry")?;
        ids.push(id);
    }
    Ok(ids)
}

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
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
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
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
        let path = args.allowlist.clone().unwrap_or_else(|| {
            key_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("authorized_keys")
        });
        let ids = load_allowlist(&path)?;
        if ids.is_empty() {
            warn!(
                "allowlist empty, denying all clients. Add keys to {} or use --allow-all",
                path.display()
            );
        } else {
            info!(
                "loaded {} allowed client(s) from {}",
                ids.len(),
                path.display()
            );
        }
        Some(ids)
    };

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;

    endpoint.online().await;
    let addr = endpoint.addr();
    info!("listening. EndpointAddr: {addr:?}");
    info!(
        "clients can connect with: zero-client --endpoint {}",
        endpoint_id
    );

    let shell = args.shell.clone().unwrap_or_else(default_shell);
    info!("shell: {shell}");

    let endpoint_for_shutdown = endpoint.clone();
    tokio::select! {
        _ = shutdown_signal() => {
            info!("shutdown signal received");
        }
        _ = accept_loop(&endpoint, allowlist, shell) => {
            info!("endpoint closed");
        }
    }
    endpoint_for_shutdown.close().await;
    info!("server stopped");
    Ok(())
}

async fn accept_loop(
    endpoint: &Endpoint,
    allowlist: Option<Vec<EndpointId>>,
    shell: String,
) {
    while let Some(incoming) = endpoint.accept().await {
        let allowlist = allowlist.clone();
        let shell = shell.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_incoming(incoming, allowlist, shell).await {
                error!("connection error: {e:#}");
            }
        });
    }
}

async fn handle_incoming(
    incoming: iroh::endpoint::Incoming,
    allowlist: Option<Vec<EndpointId>>,
    shell: String,
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
        zero_proto::write_msg(
            &mut send,
            &ServerMsg::Denied {
                reason: "expected Shell request".into(),
            },
        )
        .await?;
        return Ok(());
    };

    if let Some(allowed) = &allowlist {
        if !allowed.contains(&peer) {
            info!("denied {peer} (not in allowlist)");
            zero_proto::write_msg(
                &mut send,
                &ServerMsg::Denied {
                    reason: "not authorized".into(),
                },
            )
            .await?;
            return Ok(());
        }
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
        child.wait().ok().map(|s| s.exit_code() as i32).unwrap_or(-1)
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
