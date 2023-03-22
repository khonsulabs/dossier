use std::time::Duration;

use bonsaidb::{core::connection::AsyncConnection, server::ServerDatabase};

use crate::CliBackend;

pub(crate) fn launch(dossier: ServerDatabase<CliBackend>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
            println!("Compacting database");
            if let Err(err) = dossier.compact().await {
                eprintln!("Error compacting database: {err}");
            }
        }
    });
}
