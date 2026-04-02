use std::{env, process};

use calloop::signals::{Signal, Signals};
use clap::Parser;
use stele::{Error, IpcMessage, Stele};
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

use crate::cli::{MsgSubcommands, Options, Subcommands};

mod cli;
mod ipc_server;

fn main() {
    // Setup logging.
    let directives = env::var("RUST_LOG").unwrap_or("warn,stele=info".into());
    let env_filter = EnvFilter::builder().parse_lossy(directives);
    FmtSubscriber::builder().with_env_filter(env_filter).with_line_number(true).init();

    // Handle CLI arguments.
    let options = Options::parse();
    match options.subcommands {
        // Handle `stele msg config`.
        Some(Subcommands::Msg(MsgSubcommands::Config(config))) => match options.socket_path {
            Some(path) => {
                if let Err(err) = stele_ipc::send_message_to(path, &IpcMessage::Config(config)) {
                    error!("Failed to send IPC message: {err}");
                }
            },
            None => stele_ipc::send_message(&IpcMessage::Config(config)),
        },
        // Handle `stele msg module`.
        Some(Subcommands::Msg(MsgSubcommands::Module(module))) => match options.socket_path {
            Some(path) => {
                if let Err(err) = stele_ipc::send_message_to(path, &IpcMessage::Module(module)) {
                    error!("Failed to send IPC message: {err}");
                }
            },
            None => stele_ipc::send_message(&IpcMessage::Module(module)),
        },
        // Start Stele if no subcommand was specified.
        None => {
            if let Err(err) = run(options) {
                error!("[CRITICAL] {err}");
                process::exit(1);
            }
        },
    }
}

fn run(options: Options) -> Result<(), Error> {
    info!("Started Stele");

    let stele = Stele::new()?;

    // Setup SIGTERM handler for clean shutdown.
    let signals = Signals::new(&[Signal::SIGTERM, Signal::SIGINT])?;
    stele.event_loop().insert_source(signals, |_, _, state| state.shutdown())?;

    // Start listening on the IPC socket.
    ipc_server::spawn_ipc_socket(&stele.event_loop(), options.socket_path)?;

    stele.run()
}
