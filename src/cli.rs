use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use bonsaidb::{
    cli::CommandLine,
    core::{
        async_trait::async_trait,
        connection::{AsyncConnection, AsyncStorageConnection},
        permissions::{
            bonsai::{BonsaiAction, ServerAction},
            Statement,
        },
        schema::{NamedCollection, NamedReference, SerializedCollection},
    },
    local::config::Builder,
    server::{CustomServer, ServerConfiguration},
    AnyServerConnection,
};
use bonsaidb_files::{
    direct::{Async, File},
    FileConfig, Truncate,
};
use clap::Subcommand;
use parking_lot::Mutex;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::{
    schema::{ApiToken, Dossier, DossierFiles, Metadata, Project},
    webserver, CliBackend,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Cli {
    #[clap(subcommand)]
    Project(ProjectCommand),
    #[clap(subcommand)]
    ApiToken(ApiTokenCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectCommand {
    Create {
        slug: String,
    },
    List,
    Upload {
        location: PathBuf,
        remote_path: String,
        #[clap(long("api-token"), env("API_TOKEN"))]
        api_token: u64,
    },
    Sync {
        location: PathBuf,
        remote_path: String,
        #[clap(long("api-token"), env("API_TOKEN"))]
        api_token: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ApiTokenCommand {
    Create { slug: String },
    Delete { token: u64 },
    List,
}

#[async_trait]
impl CommandLine for CliBackend {
    type Backend = Self;
    type Subcommand = Cli;

    async fn configuration(&mut self) -> anyhow::Result<ServerConfiguration<CliBackend>> {
        Ok(ServerConfiguration::new("dossier.bonsaidb")
            .default_permissions(
                Statement::for_any().allowing(&BonsaiAction::Server(ServerAction::Connect)),
            )
            .with_schema::<Dossier>()?)
    }

    async fn open_server(&mut self) -> anyhow::Result<CustomServer<Self::Backend>> {
        let server = CustomServer::<Self::Backend>::open(self.configuration().await?).await?;

        let dossier = server.create_database::<Dossier>("dossier", true).await?;

        webserver::launch(dossier);

        Ok(server)
    }

    async fn execute(
        &mut self,
        command: Self::Subcommand,
        connection: AnyServerConnection<Self>,
    ) -> anyhow::Result<()> {
        let database = connection.database::<Dossier>("dossier").await?;
        match command {
            Cli::Project(ProjectCommand::Create { slug }) => {
                let new_project = Project { slug }.push_into_async(&database).await?;
                println!("Project #{} created.", new_project.header.id);
            }
            Cli::Project(ProjectCommand::List) => {
                let mut projects = Project::all_async(&database).await?;
                projects.sort_by(|a, b| a.contents.slug.cmp(&b.contents.slug));
                for project in projects {
                    println!("{}", project.contents.slug);
                }
            }
            Cli::Project(ProjectCommand::Upload {
                location,
                remote_path,
                api_token,
            }) => upload_file(location, remote_path, api_token, None, &database).await?,
            Cli::Project(ProjectCommand::Sync {
                location,
                remote_path,
                api_token,
            }) => sync_directory(location, remote_path, api_token, &database).await?,
            Cli::ApiToken(ApiTokenCommand::Create { slug }) => {
                let project_id = NamedReference::from(&slug)
                    .id_async::<Project, _>(&database)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("project {} not found", slug))?;

                let token = ApiToken::create(project_id, &database).await?;
                println!("Token {} created.", token.header.id);
            }
            Cli::ApiToken(ApiTokenCommand::Delete { token }) => {
                let token = ApiToken::get_async(token, &database)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Token {} not found", token))?;

                token.delete_async(&database).await?;
                println!("Token {} deleted", token.header.id)
            }
            Cli::ApiToken(ApiTokenCommand::List) => {
                let mut tokens = ApiToken::all_async(&database).await?;
                tokens.sort_by(
                    |a, b| match a.contents.project_id.cmp(&b.contents.project_id) {
                        Ordering::Equal => a.header.id.cmp(&b.header.id),
                        other => other,
                    },
                );
                let project_ids = tokens
                    .iter()
                    .map(|token| token.contents.project_id)
                    .collect::<HashSet<_>>();
                let projects = Project::get_multiple_async(project_ids, &database).await?;
                for token in tokens {
                    if let Some(project) = projects
                        .iter()
                        .find(|project| project.header.id == token.contents.project_id)
                    {
                        println!("{}: {}", project.contents.slug, token.header.id);
                    } else {
                        println!("{}: {}", token.contents.project_id, token.header.id);
                    }
                }
            }
        }
        Ok(())
    }
}

async fn upload_file<Database: AsyncConnection + Clone + 'static>(
    location: PathBuf,
    mut remote_path: String,
    api_token: u64,
    existing_hash: Option<[u8; 32]>,
    database: &Database,
) -> anyhow::Result<()> {
    // Verify the API token
    if !remote_path.starts_with('/') {
        remote_path.insert(0, '/');
    }
    let slug = remote_path.split('/').nth(1).unwrap();
    let project = Project::load_async(slug, database)
        .await?
        .ok_or_else(|| anyhow::anyhow!("project {} not found", slug))?;
    let api_token = ApiToken::get_async(api_token, database)
        .await?
        .ok_or_else(|| anyhow::anyhow!("api token not found"))?;
    if project.header.id != api_token.contents.project_id {
        anyhow::bail!("api token is for different project");
    }

    if remote_path.ends_with('/') {
        let name = location
            .file_name()
            .and_then(|osstr| osstr.to_str())
            .ok_or_else(|| anyhow::anyhow!("project {} not found", slug))?;
        remote_path.push_str(name);
    }

    let file = if let Some(existing_hash) = existing_hash {
        if let Some(file) = DossierFiles::load_async(&remote_path, database.clone()).await? {
            if let Some(metadata) = file.metadata() {
                if metadata.blake3 == existing_hash {
                    anyhow::bail!("existing hash does not match");
                }
            }
            file.truncate(0, Truncate::RemovingEnd).await?;
            Some(file)
        } else {
            None
        }
    } else {
        None
    };

    let mut file = match file {
        Some(file) => file,
        None => {
            DossierFiles::build(&remote_path)
                .create_async(database.clone())
                .await?
        }
    };

    let mut writer = file.append_buffered();
    let mut reader = fs::File::open(location).await?;

    let mut scratch = [0; 16 * 1024];
    let mut sha = blake3::Hasher::new();
    loop {
        let bytes_read = reader.read(&mut scratch).await?;
        if bytes_read == 0 {
            break;
        } else {
            sha.update(&scratch[..bytes_read]);
            writer.write_all(&scratch[..bytes_read]).await?;
        }
    }
    tokio::io::copy(&mut reader, &mut writer).await?;
    writer.shutdown().await?;
    drop(writer);

    file.update_metadata(Metadata {
        blake3: sha.finalize().try_into()?,
    })
    .await?;
    println!("File uploaded to {remote_path}");
    Ok(())
}

async fn sync_directory<Database: AsyncConnection + Clone + 'static>(
    location: PathBuf,
    mut remote_path: String,
    api_token: u64,
    database: &Database,
) -> anyhow::Result<()> {
    if !location.is_dir() {
        anyhow::bail!("sync can only be used with directories");
    }

    let (hash_sender, hash_receiver) = flume::unbounded();

    if !remote_path.starts_with('/') {
        remote_path.insert(0, '/');
    }

    if !remote_path.ends_with('/') {
        remote_path.push('/');
    }

    let mut existing_files = DossierFiles::list_recursive_async(&remote_path, database)
        .await?
        .into_iter()
        .map(|file| (file.path(), file))
        .collect::<HashMap<_, _>>();
    let directories = Arc::new(Mutex::new(vec![(location, remote_path)]));

    for _ in 0..std::thread::available_parallelism().unwrap().get() * 2 {
        tokio::task::spawn(hash_directories(directories.clone(), hash_sender.clone()));
    }
    drop(hash_sender);

    let (operation_sender, operation_receiver) = flume::unbounded();
    let mut total_operations = 0;
    while let Ok(result) = hash_receiver.recv_async().await {
        let file_hash = result?;
        if let Some(existing) = existing_files.remove(&file_hash.remote_path) {
            let hash_matches = existing
                .metadata()
                .map(|metadata| metadata.blake3 == file_hash.blake3)
                .unwrap_or_default();
            if !hash_matches {
                total_operations += 1;
                operation_sender.send(SyncOperation::Replace(file_hash))?;
            }
        } else {
            total_operations += 1;
            operation_sender.send(SyncOperation::Create(file_hash))?;
        }
    }

    for (_, file_to_delete) in existing_files {
        total_operations += 1;
        operation_sender.send(SyncOperation::Delete(file_to_delete))?;
    }
    drop(operation_sender);

    let (result_sender, result_receiver) = flume::unbounded();
    for _ in 0..std::thread::available_parallelism().unwrap().get() * 2 {
        tokio::task::spawn(perform_sync_operations(
            operation_receiver.clone(),
            result_sender.clone(),
            api_token,
            database.clone(),
        ));
    }
    drop(result_sender);

    let mut completed_operations = 0;
    while let Ok(result) = result_receiver.recv_async().await {
        let file_written = result?;
        completed_operations += 1;
        println!("{file_written} ({completed_operations}/{total_operations})");
    }

    Ok(())
}

enum SyncOperation<Database: AsyncConnection + Clone> {
    Create(FileHash),
    Replace(FileHash),
    Delete(File<Async<Database>, DossierFiles>),
}

pub struct FileHash {
    path: PathBuf,
    remote_path: String,
    blake3: [u8; 32],
}

async fn hash_directories(
    directories: Arc<Mutex<Vec<(PathBuf, String)>>>,
    result_sender: flume::Sender<anyhow::Result<FileHash>>,
) {
    loop {
        let (directory, remote_path) = {
            let mut directories = directories.lock();
            match directories.pop() {
                Some(directory) => directory,
                None => break,
            }
        };

        if let Err(err) =
            check_directory(directory, remote_path, &directories, &result_sender).await
        {
            drop(result_sender.send(Err(err)));
            break;
        }
    }
}

async fn check_directory(
    directory: PathBuf,
    remote_path: String,
    directories: &Mutex<Vec<(PathBuf, String)>>,
    result_sender: &flume::Sender<anyhow::Result<FileHash>>,
) -> anyhow::Result<()> {
    let mut contents = tokio::fs::read_dir(&directory).await?;
    while let Some(entry) = contents.next_entry().await? {
        let file_type = entry.file_type().await?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(name) => {
                eprintln!("Skipping {name:?} due to path containing invalid UTF-8 characters");
                continue;
            }
        };

        if file_type.is_dir() {
            let destination_path = format!("{remote_path}{name}/");
            let mut directories = directories.lock();
            directories.push((entry.path(), destination_path));
        } else {
            let remote_path = format!("{remote_path}{name}");
            let path = entry.path();

            let mut hasher = blake3::Hasher::new();
            let mut file = fs::File::open(&path).await?;
            let mut scratch = [0; 16 * 1024];
            loop {
                let bytes_read = file.read(&mut scratch).await?;
                if bytes_read > 0 {
                    hasher.update(&scratch[..bytes_read]);
                } else {
                    break;
                }
            }
            result_sender.send(Ok(FileHash {
                path,
                remote_path,
                blake3: hasher.finalize().try_into().unwrap(),
            }))?;
        }
    }

    Ok(())
}

async fn perform_sync_operations<Database: AsyncConnection + Clone + 'static>(
    operations: flume::Receiver<SyncOperation<Database>>,
    result_sender: flume::Sender<anyhow::Result<String>>,
    api_token: u64,
    database: Database,
) {
    while let Ok(op) = operations.recv_async().await {
        if result_sender
            .send(perform_sync_operation(op, api_token, &database).await)
            .is_err()
        {
            break;
        }
    }
}

async fn perform_sync_operation<Database: AsyncConnection + Clone + 'static>(
    operation: SyncOperation<Database>,
    api_token: u64,
    database: &Database,
) -> anyhow::Result<String> {
    match operation {
        SyncOperation::Create(file_hash) => {
            upload_file(
                file_hash.path,
                file_hash.remote_path.clone(),
                api_token,
                None,
                database,
            )
            .await?;
            Ok(file_hash.remote_path)
        }
        SyncOperation::Replace(file_hash) => {
            upload_file(
                file_hash.path,
                file_hash.remote_path.clone(),
                api_token,
                Some(file_hash.blake3),
                database,
            )
            .await?;

            Ok(file_hash.remote_path)
        }
        SyncOperation::Delete(file) => {
            file.delete().await?;
            Ok(file.path())
        }
    }
}
