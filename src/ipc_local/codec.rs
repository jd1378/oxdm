//! Length-prefixed JSON framing.
//!
//! Each frame is a 4-byte big-endian `u32` length followed by that
//! many bytes of UTF-8 JSON. Both sides use this layout symmetrically
//! over `interprocess::local_socket::tokio::Stream`.

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_FRAME: usize = 32 * 1024 * 1024; // 32 MiB safety cap.

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large: {0} bytes")]
    TooLarge(u32),
    #[error("connection closed")]
    Closed,
}

pub async fn write_frame<W, T>(w: &mut W, value: &T) -> Result<(), CodecError>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(CodecError::TooLarge(bytes.len() as u32));
    }
    let len = (bytes.len() as u32).to_be_bytes();
    w.write_all(&len).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R, T>(r: &mut R) -> Result<T, CodecError>
where
    R: AsyncReadExt + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(CodecError::Closed);
        }
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf);
    if len as usize > MAX_FRAME {
        return Err(CodecError::TooLarge(len));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}
