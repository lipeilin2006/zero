use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::{execute, terminal};
use iroh::{Endpoint, EndpointId, SecretKey, endpoint::presets};
use tokio::sync::mpsc;
use zero_proto::{ALPN, ClientMsg, ServerMsg};

#[derive(Parser, Debug)]
#[command(name = "zero", about = "console client for zero-server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// 兼容旧用法：zero <endpoint 或已保存的名称>
    target: Option<String>,

    /// Path to the persistent client secret key
    #[arg(long)]
    key: Option<PathBuf>,

    /// Override initial terminal columns
    #[arg(long)]
    cols: Option<u16>,

    /// Override initial terminal rows
    #[arg(long)]
    rows: Option<u16>,

    /// Print this client's endpoint id and exit
    #[arg(long)]
    show_id: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 连接服务器（等价于 zero <目标>）
    Connect {
        /// endpoint id 或已保存的名称
        target: String,
        #[arg(long)]
        key: Option<PathBuf>,
        #[arg(long)]
        cols: Option<u16>,
        #[arg(long)]
        rows: Option<u16>,
    },

    /// 保存一台服务器
    Add {
        name: String,
        endpoint: String,
        /// 标签，逗号分隔
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },

    /// 列出保存的服务器
    List {
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 删除保存的服务器（支持名称 / endpoint / 前缀）
    #[command(visible_aliases = ["rm", "delete"])]
    Remove { target: String },

    /// 查看某台服务器的完整 endpoint
    Show { target: String },
}

struct TerminalGuard;

#[derive(Default)]
struct TerminalModes {
    application_cursor: bool,
    bracketed_paste: bool,
    mouse: bool,
    sgr_mouse: bool,
    pending: Vec<u8>,
}

enum OutputState {
    Normal,
    Escape,
    Csi(Vec<u8>),
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
}

struct TerminalOutput {
    state: OutputState,
}

impl Default for TerminalOutput {
    fn default() -> Self {
        Self {
            state: OutputState::Normal,
        }
    }
}

impl TerminalOutput {
    fn feed(&mut self, data: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut visible = Vec::new();
        let mut replies = Vec::new();
        for &byte in data {
            let state = std::mem::replace(&mut self.state, OutputState::Normal);
            self.state = match state {
                OutputState::Normal => {
                    if byte == 0x1b {
                        OutputState::Escape
                    } else {
                        visible.push(byte);
                        OutputState::Normal
                    }
                }
                OutputState::Escape => match byte {
                    b'[' => OutputState::Csi(Vec::new()),
                    b']' => OutputState::Osc(Vec::new()),
                    _ => {
                        visible.extend_from_slice(&[0x1b, byte]);
                        OutputState::Normal
                    }
                },
                OutputState::Csi(mut sequence) => {
                    sequence.push(byte);
                    if (0x40..=0x7e).contains(&byte) {
                        if let Some(reply) = csi_query_reply(&sequence) {
                            replies.push(reply);
                        } else {
                            visible.extend_from_slice(b"\x1b[");
                            visible.extend_from_slice(&sequence);
                        }
                        OutputState::Normal
                    } else {
                        OutputState::Csi(sequence)
                    }
                }
                OutputState::Osc(mut sequence) => {
                    if byte == 0x07 {
                        if let Some(reply) = osc_query_reply(&sequence) {
                            replies.push(reply);
                        } else {
                            visible.extend_from_slice(b"\x1b]");
                            visible.extend_from_slice(&sequence);
                            visible.push(byte);
                        }
                        OutputState::Normal
                    } else if byte == 0x1b {
                        OutputState::OscEscape(sequence)
                    } else {
                        sequence.push(byte);
                        OutputState::Osc(sequence)
                    }
                }
                OutputState::OscEscape(mut sequence) => {
                    if byte == b'\\' {
                        if let Some(reply) = osc_query_reply(&sequence) {
                            replies.push(reply);
                        } else {
                            visible.extend_from_slice(b"\x1b]");
                            visible.extend_from_slice(&sequence);
                            visible.extend_from_slice(b"\x1b\\");
                        }
                        OutputState::Normal
                    } else {
                        sequence.push(0x1b);
                        sequence.push(byte);
                        OutputState::Osc(sequence)
                    }
                }
            };
        }
        (visible, replies)
    }
}

fn csi_query_reply(sequence: &[u8]) -> Option<Vec<u8>> {
    match sequence {
        b"6n" => Some(b"\x1b[1;1R".to_vec()),
        b"5n" => Some(b"\x1b[0n".to_vec()),
        b"c" | b"0c" => Some(b"\x1b[?1;2c".to_vec()),
        b">c" => Some(b"\x1b[>0;10;1c".to_vec()),
        _ => None,
    }
}

fn osc_query_reply(sequence: &[u8]) -> Option<Vec<u8>> {
    match sequence {
        b"10;?" => Some(b"\x1b]10;rgb:ffff/ffff/ffff\x07".to_vec()),
        b"11;?" => Some(b"\x1b]11;rgb:0000/0000/0000\x07".to_vec()),
        _ => None,
    }
}

impl TerminalModes {
    fn feed(&mut self, data: &[u8]) {
        self.pending.extend_from_slice(data);
        let data = std::mem::take(&mut self.pending);
        let mut index = 0;
        while index + 5 < data.len() {
            if data[index..].starts_with(b"\x1b[?") {
                let rest = &data[index + 3..];
                if let Some(end) = rest.iter().position(|byte| *byte == b'h' || *byte == b'l') {
                    let enabled = rest[end] == b'h';
                    let params = &rest[..end];
                    for parameter in params.split(|byte| *byte == b';') {
                        match parameter {
                            b"1" => self.application_cursor = enabled,
                            b"1000" | b"1002" | b"1003" => self.mouse = enabled,
                            b"1006" => self.sgr_mouse = enabled,
                            b"2004" => self.bracketed_paste = enabled,
                            _ => {}
                        }
                    }
                    index += end + 4;
                    continue;
                }
            }
            index += 1;
        }
        let keep_from = data.len().saturating_sub(8);
        self.pending.extend_from_slice(&data[keep_from..]);
    }
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            terminal::EnterAlternateScreen,
            crossterm::event::EnableMouseCapture,
            crossterm::event::EnableBracketedPaste,
            crossterm::cursor::Show,
            terminal::Clear(terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )?;
        stdout.flush()?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::cursor::Show,
            crossterm::style::ResetColor
        );
    }
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
        anyhow::bail!("odd hex length")
    }
    (0..value.len())
        .step_by(2)
        .map(|index| Ok(u8::from_str_radix(&value[index..index + 2], 16)?))
        .collect()
}

fn load_or_create_secret(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        let value = std::fs::read_to_string(path)?.trim().to_owned();
        if let Ok(bytes) = hex_decode(&value) {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("client key must contain 32 bytes"))?;
            return Ok(SecretKey::from_bytes(&bytes));
        }
        return value.parse().context("parse client secret key");
    }

    let secret = SecretKey::generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, hex_encode(&secret.to_bytes()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(secret)
}

fn modifier_param(modifiers: KeyModifiers) -> u8 {
    1 + u8::from(modifiers.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(modifiers.contains(KeyModifiers::ALT))
        + 4 * u8::from(modifiers.contains(KeyModifiers::CONTROL))
}

fn encode_key(key: KeyEvent, modes: &TerminalModes) -> Vec<u8> {
    let modified = key
        .modifiers
        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(character) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = character.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    return vec![lower as u8 - b'a' + 1];
                }
                if character == ' ' {
                    return vec![0];
                }
            }
            let mut bytes = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
            }
            let mut encoded = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            bytes
        }
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor_key(b'A', key.modifiers, modified, modes.application_cursor),
        KeyCode::Down => cursor_key(b'B', key.modifiers, modified, modes.application_cursor),
        KeyCode::Right => cursor_key(b'C', key.modifiers, modified, modes.application_cursor),
        KeyCode::Left => cursor_key(b'D', key.modifiers, modified, modes.application_cursor),
        KeyCode::Home => cursor_key(b'H', key.modifiers, modified, modes.application_cursor),
        KeyCode::End => cursor_key(b'F', key.modifiers, modified, modes.application_cursor),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(number) => function_key(number),
        _ => Vec::new(),
    }
}

fn encode_mouse(mouse: MouseEvent, modes: &TerminalModes) -> Vec<u8> {
    if !modes.mouse || !modes.sgr_mouse {
        return Vec::new();
    }
    let modifiers = mouse.modifiers;
    let modifier = u16::from(modifiers.contains(KeyModifiers::SHIFT)) * 4
        + u16::from(modifiers.contains(KeyModifiers::ALT)) * 8
        + u16::from(modifiers.contains(KeyModifiers::CONTROL)) * 16;
    let (button, release) = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => (0, false),
        MouseEventKind::Down(MouseButton::Middle) => (1, false),
        MouseEventKind::Down(MouseButton::Right) => (2, false),
        MouseEventKind::Up(_) => (3, true),
        MouseEventKind::Drag(MouseButton::Left) => (32, false),
        MouseEventKind::Drag(MouseButton::Middle) => (33, false),
        MouseEventKind::Drag(MouseButton::Right) => (34, false),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        _ => return Vec::new(),
    };
    let suffix = if release { 'm' } else { 'M' };
    format!(
        "\x1b[<{};{};{}{}",
        button + modifier,
        mouse.column + 1,
        mouse.row + 1,
        suffix
    )
    .into_bytes()
}

fn cursor_key(
    final_byte: u8,
    modifiers: KeyModifiers,
    modified: bool,
    application: bool,
) -> Vec<u8> {
    if modified {
        csi_key(final_byte, modifiers, true)
    } else if application {
        vec![0x1b, b'O', final_byte]
    } else {
        csi_key(final_byte, modifiers, false)
    }
}

fn csi_key(final_byte: u8, modifiers: KeyModifiers, modified: bool) -> Vec<u8> {
    if modified {
        format!("\x1b[1;{}{}", modifier_param(modifiers), final_byte as char).into_bytes()
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn function_key(number: u8) -> Vec<u8> {
    match number {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5..=12 => format!(
            "\x1b[{}~",
            [15, 17, 18, 19, 20, 21, 23, 24][number as usize - 5]
        )
        .into_bytes(),
        _ => Vec::new(),
    }
}

fn spawn_event_reader(tx: mpsc::UnboundedSender<Event>) {
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.show_id {
        let secret = load_or_create_secret(&cli.key.clone().unwrap_or_else(key_path))?;
        println!("{}", secret.public());
        return Ok(());
    }

    match cli.command {
        Some(Command::Add {
            name,
            endpoint,
            tags,
        }) => cmd_add(&name, &endpoint, tags),
        Some(Command::List { json }) => cmd_list(json),
        Some(Command::Remove { target }) => cmd_remove(&target),
        Some(Command::Show { target }) => cmd_show(&target),
        Some(Command::Connect {
            target,
            key,
            cols,
            rows,
        }) => connect(&target, key, cols, rows).await,
        None => match cli.target {
            Some(target) => connect(&target, cli.key, cli.cols, cli.rows).await,
            None => {
                use clap::CommandFactory;
                Cli::command().print_help()?;
                println!();
                Ok(())
            }
        },
    }
}

/// 目标可以是保存的名称、endpoint id，或 endpoint 前 8+ 位。
fn resolve_target(target: &str) -> Result<(String, Option<String>)> {
    if let Some(server) = zero_store::find(target)? {
        let _ = zero_store::touch(target);
        return Ok((server.endpoint, Some(server.name)));
    }
    parse_endpoint(target).with_context(|| {
        format!("找不到「{target}」：既不是保存的名称，也不是合法的 endpoint id")
    })?;
    Ok((target.trim().to_string(), None))
}

fn cmd_add(name: &str, endpoint: &str, tags: Vec<String>) -> Result<()> {
    parse_endpoint(endpoint).context("endpoint 格式不对")?;
    let server = zero_store::add(name, endpoint.trim(), tags)?;
    println!("已保存：{}", server.name);
    println!("endpoint：{}", server.endpoint);
    if !server.tags.is_empty() {
        println!("标签：{}", server.tags.join(", "));
    }
    println!("列表文件：{}", zero_store::store_path().display());
    Ok(())
}

fn human_age(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        0..=59 => "刚刚".to_string(),
        60..=3599 => format!("{} 分钟前", seconds / 60),
        3600..=86_399 => format!("{} 小时前", seconds / 3600),
        _ => format!("{} 天前", seconds / 86_400),
    }
}

fn short_endpoint(endpoint: &str) -> String {
    if endpoint.len() > 12 {
        format!("{}…", &endpoint[..12])
    } else {
        endpoint.to_string()
    }
}

/// 只接受裸 endpoint id。
fn parse_endpoint(value: &str) -> Result<EndpointId> {
    value
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("不是合法的 endpoint id"))
}

fn cmd_list(json: bool) -> Result<()> {
    let mut servers = zero_store::load()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&servers)?);
        return Ok(());
    }
    if servers.is_empty() {
        println!("还没有保存的服务器。");
        println!("用 zero add <名称> <endpoint> 添加，或 zero <endpoint> 直接连。");
        return Ok(());
    }
    servers.sort_by(|a, b| {
        (b.last_opened.unwrap_or(0))
            .cmp(&a.last_opened.unwrap_or(0))
            .then(a.name.cmp(&b.name))
    });
    println!("{:<16} {:<16} {:<20} 最近打开", "名称", "endpoint", "标签");
    for server in &servers {
        let tags = if server.tags.is_empty() {
            "-".to_string()
        } else {
            server.tags.join(",")
        };
        let opened = server
            .last_opened
            .map(human_age)
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<16} {:<16} {:<20} {}",
            server.name,
            short_endpoint(&server.endpoint),
            tags,
            opened
        );
    }
    println!();
    println!("共 {} 个；连接：zero <名称>", servers.len());
    Ok(())
}

fn cmd_remove(target: &str) -> Result<()> {
    let removed = zero_store::remove(target)?;
    println!(
        "已删除：{}（{}）",
        removed.name,
        short_endpoint(&removed.endpoint)
    );
    Ok(())
}

fn cmd_show(target: &str) -> Result<()> {
    let Some(server) = zero_store::find(target)? else {
        bail!("找不到「{target}」，用 zero list 看看有哪些");
    };
    println!("名称：{}", server.name);
    println!("endpoint：{}", server.endpoint);
    println!(
        "标签：{}",
        if server.tags.is_empty() {
            "-".to_string()
        } else {
            server.tags.join(", ")
        }
    );
    println!(
        "最近打开：{}",
        server
            .last_opened
            .map(human_age)
            .unwrap_or_else(|| "从未".to_string())
    );
    Ok(())
}

async fn connect(
    target: &str,
    key: Option<PathBuf>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<()> {
    let (endpoint_value, name) = resolve_target(target)?;
    let secret = load_or_create_secret(&key.unwrap_or_else(key_path))?;
    let server_id = parse_endpoint(&endpoint_value)?;

    if let Some(name) = name {
        eprintln!("connecting to {name}...");
    } else {
        eprintln!("connecting to {endpoint_value}...");
    }
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await?;
    endpoint.online().await;

    // 裸 endpoint id 要靠发现服务拿地址，刚启动时可能还没有记录，重试一下。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(35);
    let connection = loop {
        match endpoint.connect(server_id, ALPN).await {
            Ok(connection) => break connection,
            Err(error) => {
                if std::time::Instant::now() >= deadline {
                    return Err(error).context("连接失败；如果服务器刚启动，可以等几秒再试");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    };
    let (mut send, mut recv) = connection.open_bi().await?;
    let (terminal_cols, terminal_rows) =
        terminal::size().context("zero must run inside a terminal")?;
    let cols = cols.unwrap_or(terminal_cols).max(1);
    let rows = rows.unwrap_or(terminal_rows).max(1);
    zero_proto::write_msg(&mut send, &ClientMsg::Shell { cols, rows }).await?;

    match zero_proto::read_msg::<_, ServerMsg>(&mut recv).await? {
        Some(ServerMsg::Granted) => {}
        Some(ServerMsg::Denied { reason }) => anyhow::bail!("server denied access: {reason}"),
        _ => anyhow::bail!("unexpected server handshake response"),
    }

    let _terminal = TerminalGuard::enter()?;
    let (tx_event, mut rx_event) = mpsc::unbounded_channel();
    spawn_event_reader(tx_event);
    let mut stdout = io::stdout();
    let mut modes = TerminalModes::default();
    let mut output = TerminalOutput::default();

    loop {
        tokio::select! {
            event = rx_event.recv() => {
                match event {
                    Some(Event::Key(key)) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(']') {
                            let _ = zero_proto::write_msg(&mut send, &ClientMsg::Disconnect).await;
                            break;
                        }
                        let data = encode_key(key, &modes);
                        if !data.is_empty() {
                            zero_proto::write_msg(&mut send, &ClientMsg::Input { data }).await?;
                        }
                    }
                    Some(Event::Resize(cols, rows)) => {
                        zero_proto::write_msg(&mut send, &ClientMsg::Resize { cols, rows }).await?;
                    }
                    Some(Event::Paste(text)) => {
                        let mut data = Vec::with_capacity(text.len());
                        if modes.bracketed_paste {
                            data.extend_from_slice(b"\x1b[200~");
                        }
                        data.extend_from_slice(text.as_bytes());
                        if modes.bracketed_paste {
                            data.extend_from_slice(b"\x1b[201~");
                        }
                        zero_proto::write_msg(&mut send, &ClientMsg::Input { data }).await?;
                    }
                    Some(Event::Mouse(mouse)) => {
                        let data = encode_mouse(mouse, &modes);
                        if !data.is_empty() {
                            zero_proto::write_msg(&mut send, &ClientMsg::Input { data }).await?;
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            message = zero_proto::read_msg::<_, ServerMsg>(&mut recv) => {
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        // 服务器收尾时会把连接关掉，这不是致命错误。
                        eprintln!("连接已断开：{error}");
                        break;
                    }
                };
                match message {
                    Some(ServerMsg::Output { data }) => {
                        modes.feed(&data);
                        let (visible, replies) = output.feed(&data);
                        for reply in replies {
                            zero_proto::write_msg(&mut send, &ClientMsg::Input { data: reply })
                                .await?;
                        }
                        stdout.write_all(&visible)?;
                        stdout.flush()?;
                    }
                    Some(ServerMsg::Exit { .. }) | None => break,
                    Some(_) => {}
                }
            }
        }
    }

    connection.close(0u8.into(), b"bye");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_cursor_and_device_queries() {
        let mut output = TerminalOutput::default();
        let (visible, replies) = output.feed(b"hello\x1b[6n\x1b[>cworld");
        assert_eq!(visible, b"helloworld");
        assert_eq!(
            replies,
            vec![b"\x1b[1;1R".to_vec(), b"\x1b[>0;10;1c".to_vec()]
        );
    }

    #[test]
    fn filters_osc_queries_across_chunks() {
        let mut output = TerminalOutput::default();
        let (first, first_replies) = output.feed(b"\x1b]10;");
        let (second, second_replies) = output.feed(b"?\x07text");
        assert!(first.is_empty());
        assert!(first_replies.is_empty());
        assert_eq!(second, b"text");
        assert_eq!(
            second_replies,
            vec![b"\x1b]10;rgb:ffff/ffff/ffff\x07".to_vec()]
        );
    }
}
