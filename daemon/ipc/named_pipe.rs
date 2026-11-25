//! Windows Named Pipe (`\\.\pipe\onehundo`) IPC listener abstraction.
//! Provides cross-platform parity with `unix_socket.rs` on Windows targets.

pub const DEFAULT_WIN32_PIPE_NAME: &str = r"\\.\pipe\onehundo";

#[cfg(not(windows))]
pub fn windows_pipe_available() -> bool {
    false
}

#[cfg(windows)]
pub fn windows_pipe_available() -> bool {
    true
}
