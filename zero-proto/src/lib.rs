use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"zero-ssh/1";

/// Client → Server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Request an interactive shell
    Shell { cols: u16, rows: u16 },
    /// Keystrokes / pasted input
    Input { data: Vec<u8> },
    /// Terminal resized
    Resize { cols: u16, rows: u16 },
    /// Graceful disconnect
    Disconnect,
}

/// Server → Client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Shell session granted (auth ok)
    Granted,
    /// Auth / permission denied
    Denied { reason: String },
    /// Output from the remote shell
    Output { data: Vec<u8> },
    /// Shell process exited
    Exit { code: i32 },
}

/// Length-prefixed framing: [u32 LE length][payload]
pub async fn write_msg<S, T>(sink: &mut S, msg: &T) -> std::io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
    T: Serialize,
{
    use tokio::io::AsyncWriteExt;
    let payload = postcard::to_stdvec(msg).map_err(|e| std::io::Error::other(e))?;
    let len = (payload.len() as u32).to_le_bytes();
    sink.write_all(&len).await?;
    sink.write_all(&payload).await?;
    sink.flush().await
}

pub async fn read_msg<R, T>(src: &mut R) -> std::io::Result<Option<T>>
where
    R: tokio::io::AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    match src.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut payload = vec![0u8; len];
    src.read_exact(&mut payload).await?;
    let msg = postcard::from_bytes(&payload).map_err(|e| std::io::Error::other(e))?;
    Ok(Some(msg))
}
