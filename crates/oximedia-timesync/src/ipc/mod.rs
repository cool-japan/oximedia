//! Inter-process communication for time synchronization.

#[cfg(not(target_arch = "wasm32"))]
pub mod shmem;
#[cfg(unix)]
pub mod socket;

use serde::{Deserialize, Serialize};

/// Time synchronization IPC message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimeSyncMessage {
    /// Get current time offset
    GetOffset,
    /// Offset response (nanoseconds)
    OffsetResponse(i64),
    /// Get synchronization state
    GetState,
    /// State response
    StateResponse(StateInfo),
    /// Subscribe to time updates
    Subscribe,
    /// Unsubscribe from time updates
    Unsubscribe,
    /// Time update notification
    TimeUpdate {
        /// Offset (nanoseconds)
        offset_ns: i64,
        /// Timestamp (nanoseconds since epoch)
        timestamp_ns: u64,
    },
}

/// Synchronization state information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateInfo {
    /// Whether synchronized
    pub synchronized: bool,
    /// Current offset (nanoseconds)
    pub offset_ns: i64,
    /// Frequency offset (ppb)
    pub freq_offset_ppb: f64,
    /// Source name
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = TimeSyncMessage::GetOffset;
        let serialized = serde_json::to_string(&msg).expect("should succeed in test");
        let deserialized: TimeSyncMessage =
            serde_json::from_str(&serialized).expect("should succeed in test");

        match deserialized {
            TimeSyncMessage::GetOffset => {}
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_state_info() {
        let state = StateInfo {
            synchronized: true,
            offset_ns: 1000,
            freq_offset_ppb: 10.0,
            source: "PTP".to_string(),
        };

        assert!(state.synchronized);
        assert_eq!(state.offset_ns, 1000);
    }

    /// Regression test for issue #13: the Unix-socket IPC module must only be
    /// compiled on Unix-like targets. On Unix this asserts the module is
    /// reachable; on non-Unix (e.g. Windows) the module simply does not exist
    /// and this whole test is configured out — proving the gate works because
    /// the crate compiles at all on Windows.
    #[cfg(unix)]
    #[test]
    fn test_issue_13_socket_module_gated_on_unix() {
        // Force a name resolution into the gated module. If the gating ever
        // regresses (e.g. socket.rs becomes visible on Windows), the cfg here
        // and the cfg in mod.rs must stay in lockstep, otherwise this won't
        // build on Unix.
        let sock_path = std::env::temp_dir()
            .join("oximedia-issue-13.sock")
            .display()
            .to_string();
        let _server = super::socket::TimeSyncServer::new(&sock_path);
    }
}
