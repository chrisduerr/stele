use std::collections::HashMap;
use std::ffi::OsStr;
use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::{io, mem, ptr};

pub use calloop;
use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use image::ColorType;
use smithay_client_toolkit::reexports::client::globals::{
    self, BindError, GlobalError, GlobalList,
};
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
use smithay_client_toolkit::reexports::client::protocol::wl_touch::WlTouch;
use smithay_client_toolkit::reexports::client::{
    ConnectError, Connection, DispatchError, QueueHandle,
};
pub use stele_ipc::*;
use vulkano::buffer::AllocateBufferError;
use vulkano::command_buffer::CommandBufferExecError;
use vulkano::image::AllocateImageError;
use vulkano::{Validated, ValidationError, VulkanError};

use crate::ui::window::Window;
use crate::wayland::ProtocolStates;

mod geometry;
mod ui;
mod wayland;

/// Stele bar.
pub struct Stele {
    event_loop: EventLoop<'static, State>,
    state: State,
}

impl Stele {
    /// Create a new bar.
    ///
    /// ```rust,no_run
    /// use stele::Stele;
    ///
    /// let _stele = Stele::new().unwrap();
    /// ```
    pub fn new() -> Result<Self, Error> {
        // Initialize Wayland connection.
        let connection = Connection::connect_to_env()?;
        let (globals, queue) = globals::registry_queue_init(&connection)?;

        // Initialize event loop and application state.
        let mut event_loop = EventLoop::try_new()?;
        let mut state = State::new(connection.clone(), &globals, queue.handle())?;

        // Insert wayland source into calloop loop.
        let wayland_source = WaylandSource::new(connection, queue);
        wayland_source.insert(event_loop.handle())?;

        // Roundtrip Wayland once, to retrieve output information.
        event_loop.dispatch(None, &mut state).unwrap();

        Ok(Self { event_loop, state })
    }

    /// Get a mutable reference to the runtime state.
    ///
    /// ```rust,no_run
    /// use stele::Stele;
    ///
    /// let mut stele = Stele::new().unwrap();
    ///
    /// let _state = stele.state();
    /// ```
    pub fn state(&mut self) -> &mut State {
        &mut self.state
    }

    /// Get a handle for the event loop.
    ///
    /// ```rust,no_run
    /// use stele::Stele;
    ///
    /// let stele = Stele::new().unwrap();
    ///
    /// // Schedule a callback with access to the bar state.
    /// stele.event_loop().insert_idle(|_state| {});
    /// ```
    pub fn event_loop(&self) -> LoopHandle<'static, State> {
        self.event_loop.handle()
    }

    /// Start the blocking event loop.
    ///
    /// ```rust,no_run
    /// use stele::Stele;
    ///
    /// let stele = Stele::new().unwrap();
    ///
    /// stele.run().unwrap();
    /// ```
    pub fn run(mut self) -> Result<(), Error> {
        while !self.state.terminated {
            self.event_loop.dispatch(None, &mut self.state)?;
        }
        Ok(())
    }
}

/// Runtime bar state.
pub struct State {
    protocol_states: ProtocolStates,
    queue: QueueHandle<Self>,
    connection: Connection,

    pointer: Option<WlPointer>,
    touch: Option<WlTouch>,

    pending_modules: HashMap<Arc<String>, Module>,
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
            pending_modules: Default::default(),
            terminated: Default::default(),
            pointer: Default::default(),
            window: Default::default(),
            touch: Default::default(),
        })
    }

    /// Terminate the bar.
    ///
    /// ```rust,no_run
    /// use stele::Stele;
    ///
    /// let stele = Stele::new().unwrap();
    ///
    /// stele.event_loop().insert_idle(|state| state.shutdown());
    /// ```
    pub fn shutdown(&mut self) {
        self.terminated = true;
    }

    /// Update the global configuration.
    ///
    /// ```rust,no_run
    /// use stele::{Config, Stele};
    ///
    /// let mut stele = Stele::new().unwrap();
    ///
    /// stele.state().update_config(Config::new());
    /// ```
    pub fn update_config(&mut self, config: Config) {
        match &mut self.window {
            Some(window) => window.update_config(config),
            None => {
                let modules = mem::take(&mut self.pending_modules);
                let connection = self.connection.clone();
                let queue = self.queue.clone();
                match Window::new(&self.protocol_states, connection, queue, config, modules) {
                    Ok(window) => self.window = Some(window),
                    Err(err) => panic!("Failed to initialize window: {err}"),
                }
            },
        }
    }

    /// Create, update, or delete a module.
    ///
    /// ```rust,no_run
    /// use stele::{Alignment, Module, Stele};
    ///
    /// let mut stele = Stele::new().unwrap();
    ///
    /// let module = Module::new("module_id", Alignment::Start, Vec::new());
    /// stele.state().update_module(module);
    /// ```
    pub fn update_module(&mut self, module: Module) {
        match &mut self.window {
            Some(window) => window.update_module(module),
            None => _ = self.pending_modules.insert(module.id.clone(), module),
        }
    }
}

/// Spawn an unsupervised child process.
///
/// This will double-fork to avoid spawning zombies, but does not provide any
/// ability to retrieve the process' output.
pub fn daemon<I, S>(program: S, args: I) -> io::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    unsafe {
        command.pre_exec(|| {
            // Perform second fork.
            match libc::fork() {
                -1 => return Err(io::Error::last_os_error()),
                0 => (),
                _ => libc::_exit(0),
            }

            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }

            // Reset signal handlers.
            let mut signal_set = MaybeUninit::uninit();
            libc::sigemptyset(signal_set.as_mut_ptr());
            libc::sigprocmask(libc::SIG_SETMASK, signal_set.as_mut_ptr(), ptr::null_mut());

            Ok(())
        });
    }

    command.spawn()?.wait()?;

    Ok(())
}

/// Stele error.
#[derive(thiserror::Error, Debug)]
pub enum Error {
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
    #[error("Failed allocate Vulkan image: {0}")]
    VulkanAllocateImage(#[from] Validated<AllocateImageError>),
    #[error("Failed Vulkan command execution: {0}")]
    VulkanCommandExecution(#[from] CommandBufferExecError),
    #[error("Failed to create Vulkan buffer: {0}")]
    VulkanBuffer(#[from] Validated<AllocateBufferError>),
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
    #[error("Unsupported image format: {0:?}")]
    UnsupportedImageFormat(ColorType),
    #[error("Failed to load image: {0}")]
    Image(#[from] image::ImageError),
    #[error("Failed to load SVG: {0}")]
    Svg(#[from] resvg::usvg::Error),
    #[error("Layer has no size")]
    EmptyLayer,
}

impl<T> From<calloop::InsertError<T>> for Error {
    fn from(err: calloop::InsertError<T>) -> Self {
        Self::EventLoop(err.error)
    }
}
