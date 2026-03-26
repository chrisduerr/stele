use std::{env, process};

use calloop::EventLoop;
use calloop::signals::{Signal, Signals};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::{
    self, BindError, GlobalError, GlobalList,
};
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
use smithay_client_toolkit::reexports::client::protocol::wl_touch::WlTouch;
use smithay_client_toolkit::reexports::client::{
    ConnectError, Connection, DispatchError, QueueHandle,
};
use stele_ipc::{Config, Module};
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, FmtSubscriber};
use vulkano::command_buffer::CommandBufferExecError;
use vulkano::pipeline::layout::IntoPipelineLayoutCreateInfoError;
use vulkano::{Validated, ValidationError, VulkanError};

use crate::ui::window::Window;
use crate::wayland::ProtocolStates;

mod geometry;
mod ipc_server;
mod ui;
mod wayland;

fn main() {
    // Setup logging.
    let directives = env::var("RUST_LOG").unwrap_or("warn,stele=info".into());
    let env_filter = EnvFilter::builder().parse_lossy(directives);
    FmtSubscriber::builder().with_env_filter(env_filter).with_line_number(true).init();

    info!("Started Stele");

    if let Err(err) = run() {
        error!("[CRITICAL] {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    // Initialize Wayland connection.
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = globals::registry_queue_init(&connection)?;

    let mut event_loop = EventLoop::try_new()?;
    let mut state = State::new(connection.clone(), &globals, queue.handle())?;

    // Insert wayland source into calloop loop.
    let wayland_source = WaylandSource::new(connection, queue);
    wayland_source.insert(event_loop.handle())?;

    // Setup SIGTERM handler for clean shutdown.
    let signals = Signals::new(&[Signal::SIGTERM, Signal::SIGINT])?;
    event_loop.handle().insert_source(signals, |_, _, state| state.terminated = true)?;

    // Start listening on the IPC socket.
    ipc_server::spawn_ipc_socket(&event_loop.handle())?;

    // Start event loop.
    while !state.terminated {
        event_loop.dispatch(None, &mut state)?;
    }

    Ok(())
}

/// Application state.
struct State {
    protocol_states: ProtocolStates,
    queue: QueueHandle<Self>,
    connection: Connection,

    pointer: Option<WlPointer>,
    touch: Option<WlTouch>,

    window: Option<Window>,

    terminated: bool,
}

impl State {
    fn new(
        connection: Connection,
        globals: &GlobalList,
        queue: QueueHandle<Self>,
    ) -> Result<Self, Error> {
        let protocol_states = ProtocolStates::new(globals, &queue)?;

        Ok(Self {
            protocol_states,
            connection,
            queue,
            terminated: Default::default(),
            pointer: Default::default(),
            window: Default::default(),
            touch: Default::default(),
        })
    }

    /// Update the global configuration.
    pub fn update_config(&mut self, config: Config) {
        match &mut self.window {
            Some(window) => window.update_config(config),
            None => {
                let connection = self.connection.clone();
                let queue = self.queue.clone();
                match Window::new(&self.protocol_states, connection, queue, config) {
                    Ok(window) => self.window = Some(window),
                    Err(err) => panic!("Failed to initialize window: {err}"),
                }
            },
        }
    }

    /// Create, update, or delete a module.
    pub fn update_module(&mut self, module: Module) {
        // TODO
    }
}

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("Wayland protocol error for {0}: {1}")]
    WaylandProtocol(&'static str, #[source] BindError),
    #[error("{0}")]
    WaylandDispatch(#[from] DispatchError),
    #[error("{0}")]
    WaylandConnect(#[from] ConnectError),
    #[error("{0}")]
    WaylandGlobal(#[from] GlobalError),
    #[error("{0}")]
    EventLoop(#[from] calloop::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to create Vulkan pipeline parameters: {0}")]
    VulkanPipelineParams(#[from] IntoPipelineLayoutCreateInfoError),
    #[error("Failed Vulkan command execution: {0}")]
    VulkanCommandExecution(#[from] CommandBufferExecError),
    #[error("Vulkan validation error: {0}")]
    VulkanValidationBox(#[from] Box<ValidationError>),
    #[error("Vulkan validation error: {0}")]
    VulkanValidation(#[from] Validated<VulkanError>),
    #[error("Failed to load Vulkan library: {0}")]
    VulkanLoad(#[from] vulkano::LoadingError),
    #[error("Vulkan error: {0}")]
    Vulkan(#[from] VulkanError),
    #[error("No suitable Vulkan device found")]
    VulkanNoDevice,
}

impl<T> From<calloop::InsertError<T>> for Error {
    fn from(err: calloop::InsertError<T>) -> Self {
        Self::EventLoop(err.error)
    }
}
