#![doc = include_str!("../README.md")]

mod api;
mod cli;
mod compactor;
mod permissions;
mod schema;
mod webserver;

use std::{convert::Infallible, num::NonZeroUsize};

use bonsaidb::{cli::CommandLine, core::async_trait::async_trait, server::Backend};

fn main() -> anyhow::Result<()> {
    let worker_threads = std::env::var("WORKERS")
        .ok()
        .and_then(|workers| workers.parse::<usize>().ok())
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(NonZeroUsize::get)
        })
        .unwrap_or(16);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .build()?
        .block_on(CliBackend.run())
}

#[derive(Debug, Default)]
struct CliBackend;

#[async_trait]
impl Backend for CliBackend {
    type Error = Infallible;
    type ClientData = ();
}
