//! IPC UDS socket server.

use std::io::Read;
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::{env, process};

use calloop::LoopHandle;
use stele_ipc::IpcMessage;
use tracing::{debug, error, warn};

use crate::ipc_server::socket::SocketSource;
use crate::{Error, State};

mod socket;

/// Create and listen on the socket.
pub fn spawn_ipc_socket(event_loop: &LoopHandle<'static, State>) -> Result<PathBuf, Error> {
    let socket_path = socket_path();

    // Spawn unix socket event source.
    let listener = UnixListener::bind(&socket_path)?;
    let socket = SocketSource::new(socket_path.clone(), listener)?;

    // Add source to calloop loop.
    let mut message_buffer = String::new();
    event_loop.insert_source(socket, move |stream, _, state| {
        handle_message(&mut message_buffer, stream, state);
    })?;

    Ok(socket_path)
}

/// Handle IPC socket messages.
fn handle_message(buffer: &mut String, mut stream: UnixStream, state: &mut State) {
    buffer.clear();

    // Close writer, since we're not going to write anything.
    if let Err(err) = stream.shutdown(Shutdown::Write) {
        error!("Failed to shut down socket writer: {err}");
    }

    // Read new content to buffer.
    if let Err(err) = stream.read_to_string(buffer) {
        error!("Failed to read from socket: {err}");
        return;
    }

    // Read pending events on socket.
    let message: IpcMessage = match serde_json::from_str(buffer) {
        Ok(message) => message,
        Err(err) => {
            warn!("Received invalid socket message: {err}");
            debug!("{buffer}");
            return;
        },
    };

    // Handle IPC events.
    match message {
        IpcMessage::Config(config) => state.update_config(config),
        IpcMessage::Module(module) => state.update_module(module),
    }
}

/// Get the IPC socket path.
fn socket_path() -> PathBuf {
    let pid = process::id();
    let file_name = format!("stele-{pid}.sock");
    let runtime_dir = dirs::runtime_dir().unwrap_or_else(env::temp_dir);
    runtime_dir.join(file_name)
}
