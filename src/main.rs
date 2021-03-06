#![doc = include_str!("../README.md")]

mod api;
mod cli;
mod permissions;
mod schema;
mod webserver;

use std::convert::Infallible;

use bonsaidb::{cli::CommandLine, core::async_trait::async_trait, server::Backend};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    CliBackend.run().await
}

#[derive(Debug)]
struct CliBackend;

#[async_trait]
impl Backend for CliBackend {
    type Error = Infallible;
    type ClientData = ();
}
