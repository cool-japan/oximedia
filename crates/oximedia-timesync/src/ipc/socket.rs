//! Unix domain socket IPC for time synchronization.

use super::{StateInfo, TimeSyncMessage};
use crate::error::{TimeSyncError, TimeSyncResult};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info};

#[cfg(not(unix))]
fn unsupported_platform_error() -> TimeSyncError {
    TimeSyncError::Ipc("Unix domain sockets are only supported on Unix platforms".to_string())
}

/// Unix socket server for time synchronization.
#[cfg(unix)]
pub struct TimeSyncServer {
    /// Socket path
    socket_path: PathBuf,
    /// Listener
    listener: Option<UnixListener>,
}

#[cfg(unix)]
impl TimeSyncServer {
    /// Create a new time sync server.
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            listener: None,
        }
    }

    /// Start the server.
    pub async fn start(&mut self) -> TimeSyncResult<()> {
        // Remove existing socket file if present
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        info!("Time sync server listening on {:?}", self.socket_path);

        self.listener = Some(listener);
        Ok(())
    }

    /// Accept and handle a client connection.
    pub async fn accept(&mut self) -> TimeSyncResult<UnixStream> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| TimeSyncError::Ipc("Server not started".to_string()))?;

        let (stream, _addr) = listener.accept().await?;
        debug!("Accepted client connection");
        Ok(stream)
    }

    /// Handle a client request.
    pub async fn handle_client(
        stream: UnixStream,
        mut get_state: impl FnMut() -> StateInfo,
    ) -> TimeSyncResult<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;

            if n == 0 {
                // EOF - client disconnected
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<TimeSyncMessage>(trimmed) {
                Ok(msg) => {
                    let response = match msg {
                        TimeSyncMessage::GetOffset => {
                            let state = get_state();
                            TimeSyncMessage::OffsetResponse(state.offset_ns)
                        }
                        TimeSyncMessage::GetState => {
                            let state = get_state();
                            TimeSyncMessage::StateResponse(state)
                        }
                        _ => {
                            error!("Unsupported message: {:?}", msg);
                            continue;
                        }
                    };

                    let response_json = serde_json::to_string(&response)?;
                    writer.write_all(response_json.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                    writer.flush().await?;
                }
                Err(e) => {
                    error!("Failed to parse message: {}", e);
                }
            }
        }

        Ok(())
    }
}

#[cfg(unix)]
impl Drop for TimeSyncServer {
    fn drop(&mut self) {
        // Clean up socket file
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

/// Unix socket client for time synchronization.
#[cfg(unix)]
pub struct TimeSyncClient {
    /// Socket path
    socket_path: PathBuf,
    /// Connection
    stream: Option<UnixStream>,
}

#[cfg(unix)]
impl TimeSyncClient {
    /// Create a new time sync client.
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            stream: None,
        }
    }

    /// Connect to the server.
    pub async fn connect(&mut self) -> TimeSyncResult<()> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Send a request and receive response.
    pub async fn request(&mut self, msg: TimeSyncMessage) -> TimeSyncResult<TimeSyncMessage> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| TimeSyncError::Ipc("Not connected".to_string()))?;

        // Send request
        let request_json = serde_json::to_string(&msg)?;
        stream.write_all(request_json.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;

        // Read response
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let response: TimeSyncMessage = serde_json::from_str(&line)?;
        Ok(response)
    }

    /// Get current offset.
    pub async fn get_offset(&mut self) -> TimeSyncResult<i64> {
        let response = self.request(TimeSyncMessage::GetOffset).await?;

        match response {
            TimeSyncMessage::OffsetResponse(offset) => Ok(offset),
            _ => Err(TimeSyncError::Ipc("Unexpected response".to_string())),
        }
    }

    /// Get synchronization state.
    pub async fn get_state(&mut self) -> TimeSyncResult<StateInfo> {
        let response = self.request(TimeSyncMessage::GetState).await?;

        match response {
            TimeSyncMessage::StateResponse(state) => Ok(state),
            _ => Err(TimeSyncError::Ipc("Unexpected response".to_string())),
        }
    }
}

#[cfg(not(unix))]
pub struct TimeSyncServer {
    /// Socket path
    socket_path: PathBuf,
}

#[cfg(not(unix))]
impl TimeSyncServer {
    /// Create a new time sync server.
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
        }
    }

    /// Start the server.
    pub async fn start(&mut self) -> TimeSyncResult<()> {
        let _ = &self.socket_path;
        Err(unsupported_platform_error())
    }

    /// Accept and handle a client connection.
    pub async fn accept(&mut self) -> TimeSyncResult<()> {
        let _ = &self.socket_path;
        Err(unsupported_platform_error())
    }

    /// Handle a client request.
    pub async fn handle_client(
        _get_state_stream: (),
        mut _get_state: impl FnMut() -> StateInfo,
    ) -> TimeSyncResult<()> {
        Err(unsupported_platform_error())
    }
}

#[cfg(not(unix))]
pub struct TimeSyncClient {
    /// Socket path
    socket_path: PathBuf,
}

#[cfg(not(unix))]
impl TimeSyncClient {
    /// Create a new time sync client.
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
        }
    }

    /// Connect to the server.
    pub async fn connect(&mut self) -> TimeSyncResult<()> {
        let _ = &self.socket_path;
        Err(unsupported_platform_error())
    }

    /// Send a request and receive response.
    pub async fn request(&mut self, _msg: TimeSyncMessage) -> TimeSyncResult<TimeSyncMessage> {
        let _ = &self.socket_path;
        Err(unsupported_platform_error())
    }

    /// Get current offset.
    pub async fn get_offset(&mut self) -> TimeSyncResult<i64> {
        let _ = &self.socket_path;
        Err(unsupported_platform_error())
    }

    /// Get synchronization state.
    pub async fn get_state(&mut self) -> TimeSyncResult<StateInfo> {
        let _ = &self.socket_path;
        Err(unsupported_platform_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_sock(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia-timesync-ipc-{name}"))
    }

    #[test]
    #[cfg(unix)]
    fn test_server_creation() {
        let server = TimeSyncServer::new(tmp_sock("server.sock"));
        assert!(server.listener.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn test_client_creation() {
        let client = TimeSyncClient::new(tmp_sock("client.sock"));
        assert!(client.stream.is_none());
    }

    #[test]
    #[cfg(not(unix))]
    fn test_server_creation() {
        let _server = TimeSyncServer::new(tmp_sock("server.sock"));
    }

    #[test]
    #[cfg(not(unix))]
    fn test_client_creation() {
        let _client = TimeSyncClient::new(tmp_sock("client.sock"));
    }
}
