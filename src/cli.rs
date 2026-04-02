//! CLI argument handling.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use stele::{Config, Module};

/// Command line arguments.
#[derive(Parser, Debug)]
#[clap(author, about, version, max_term_width = 80)]
pub struct Options {
    /// Path for the IPC socket.
    #[arg(long, value_name = "PATH", global = true)]
    pub socket_path: Option<PathBuf>,

    #[clap(subcommand)]
    pub subcommands: Option<Subcommands>,
}

#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Send IPC messages to Stele.
    #[clap(subcommand)]
    Msg(MsgSubcommands),
}

#[derive(Subcommand, Debug)]
pub enum MsgSubcommands {
    /// Update the global configuration.
    Config(Config),
    /// Update a module's configuration.
    Module(Module),
}
