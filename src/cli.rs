use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use bonsaidb::{
    cli::CommandLine,
    core::{
        arc_bytes::serde::Bytes,
        async_trait::async_trait,
        connection::{AsyncConnection, AsyncStorageConnection, AuthenticationMethod},
        permissions::{
            bonsai::{BonsaiAction, ServerAction},
            Statement,
        },
        schema::{NamedReference, SerializedCollection},
    },
    files::{
        direct::{Async, File},
        FileConfig,
    },
    local::config::Builder,
    server::{CustomServer, ServerConfiguration},
    AnyDatabase, AnyServerConnection,
};
use clap::Subcommand;
use parking_lot::Mutex;
use ron::ser::PrettyConfig;
use tokio::{fs, io::AsyncReadExt};

use crate::{
    api::{self, DeleteFile, DossierApiHandler, ListFiles, WriteFileData},
    permissions,
    schema::{ApiToken, Dossier, DossierFiles, Project},
    webserver, CliBackend,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Cli {
    #[clap(subcommand)]
    Project(ProjectCommand),
    #[clap(subcommand)]
    ApiToken(ApiTokenCommand),
    Compact,
    Backup {
        destination: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectCommand {
    Create {
        slug: String,
    },
    List,
    Upload {
        project: String,
        location: PathBuf,
        remote_path: String,
    },
    Sync {
        project: String,
        location: PathBuf,
        remote_path: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ApiTokenCommand {
    Create { slug: String, label: String },
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
                Statement::for_any()
                    .allowing(&BonsaiAction::Server(ServerAction::Connect))
                    .allowing(&BonsaiAction::Server(ServerAction::Authenticate(
                        AuthenticationMethod::PasswordHash,
                    )))
                    .allowing(&BonsaiAction::Server(ServerAction::Authenticate(
                        AuthenticationMethod::Token,
                    ))),
            )
            .with_schema::<Dossier>()?
            .with_api::<DossierApiHandler, ListFiles>()?
            .with_api::<DossierApiHandler, WriteFileData>()?
            .with_api::<DossierApiHandler, DeleteFile>()?)
    }

    async fn open_server(&mut self) -> anyhow::Result<CustomServer<Self::Backend>> {
        let server = CustomServer::<Self::Backend>::open(self.configuration().await?).await?;

        let dossier = server.create_database::<Dossier>("dossier", true).await?;

        permissions::initialize(&server).await?;

        webserver::launch(server.clone(), dossier);

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
                project,
            }) => upload_file(location, remote_path, &project, &database, None).await?,
            Cli::Project(ProjectCommand::Sync {
                location,
                remote_path,
                project,
            }) => sync_directory(location, remote_path, &project, &database).await?,
            Cli::ApiToken(ApiTokenCommand::Create { slug, label }) => {
                let project_id = NamedReference::from(&slug)
                    .id_async::<Project, _>(&database)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("project {} not found", slug))?;

                let (api_token, auth_token) =
                    ApiToken::create(label, project_id, &database, &connection.admin().await)
                        .await?;

                // Create a role for this token
                println!(
                    "Token {} created for {slug}: id {} - private token {}",
                    api_token.contents.label,
                    auth_token.header.id,
                    auth_token.contents.token.as_str()
                );
            }
            Cli::ApiToken(ApiTokenCommand::Delete { token }) => {
                let token = ApiToken::get_async(&token, &database)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Token {} not found", token))?;

                ApiToken::delete(&token, &database, &connection.admin().await).await?;
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
                let projects = Project::get_multiple_async(&project_ids, &database).await?;
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
            Cli::Compact => {
                database.compact().await?;
            }
            Cli::Backup { destination } => {
                backup(&database, &destination).await?;
            }
        }
        Ok(())
    }
}

async fn upload_file(
    location: PathBuf,
    mut remote_path: String,
    project: &str,
    database: &AnyDatabase<CliBackend>,
    verify_hash: Option<[u8; 32]>,
) -> anyhow::Result<()> {
    if !remote_path.starts_with('/') {
        remote_path.insert_str(0, &format!("/{project}/"));
    } else {
        remote_path.insert_str(0, &format!("/{project}"));
    }

    if remote_path.ends_with('/') {
        let name = location
            .file_name()
            .and_then(|osstr| osstr.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid file name: {:?}", location.file_name()))?;
        remote_path.push_str(name);
    }

    loop {
        let mut verify_hash = VerificationHash::new(verify_hash);
        let mut reader = fs::File::open(&location).await?;

        let mut scratch = vec![0; 1_048_576];
        let mut current_len = 0;
        let mut is_first_write = true;
        let mut file_hash = None;
        loop {
            let bytes_read = reader.read(&mut scratch[current_len..]).await?;
            current_len += bytes_read;
            if bytes_read == 0 || current_len == scratch.len() {
                file_hash = write_file_data(
                    &remote_path,
                    &scratch[..current_len],
                    is_first_write,
                    bytes_read == 0,
                    database,
                )
                .await?;
                verify_hash.update(&scratch[..current_len]);
                is_first_write = false;
                current_len = 0;
            }

            if bytes_read == 0 {
                break;
            }
        }

        if file_hash.is_none() {
            file_hash = write_file_data(&remote_path, &[], is_first_write, true, database).await?;
        }

        let verify_hash = verify_hash.finish();
        if file_hash.as_ref().unwrap().as_slice() == verify_hash {
            break;
        } else {
            println!("Upload failed to verify, trying again {remote_path}. Server: {file_hash:?}, Local: {verify_hash:?}");
        }
    }

    println!("File uploaded to {remote_path}");
    Ok(())
}

#[allow(clippy::large_enum_variant)]
enum VerificationHash {
    Static([u8; 32]),
    Computing(blake3::Hasher),
}

impl VerificationHash {
    pub fn new(verify_hash: Option<[u8; 32]>) -> Self {
        if let Some(verify_hash) = verify_hash {
            Self::Static(verify_hash)
        } else {
            Self::Computing(blake3::Hasher::new())
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        if let VerificationHash::Computing(hasher) = self {
            hasher.update(bytes);
        }
    }

    pub fn finish(self) -> [u8; 32] {
        match self {
            VerificationHash::Static(value) => value,
            VerificationHash::Computing(hasher) => *hasher.finalize().as_bytes(),
        }
    }
}

async fn sync_directory(
    location: PathBuf,
    mut remote_path: String,
    project: &str,
    database: &AnyDatabase<CliBackend>,
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

    let mut existing_files = list_files(&format!("/{project}{remote_path}"), database).await?;
    let directories = Arc::new(Mutex::new(vec![(location, remote_path)]));

    println!(
        "Computing local hashes. Remote has {} files.",
        existing_files.len()
    );
    for _ in 0..std::thread::available_parallelism().unwrap().get() * 2 {
        tokio::task::spawn(hash_directories(directories.clone(), hash_sender.clone()));
    }
    drop(hash_sender);

    let (operation_sender, operation_receiver) = flume::unbounded();
    let mut total_operations = 0;
    while let Ok(result) = hash_receiver.recv_async().await {
        let file_hash = result?;
        if let Some(existing_hash) =
            existing_files.remove(&format!("/{project}{}", file_hash.remote_path))
        {
            if existing_hash.as_slice() != file_hash.blake3 {
                total_operations += 1;
                operation_sender.send(SyncOperation::Replace(file_hash))?;
            }
        } else {
            total_operations += 1;
            operation_sender.send(SyncOperation::Create(file_hash))?;
        }
    }

    for (file_to_delete, _) in existing_files {
        total_operations += 1;
        operation_sender.send(SyncOperation::Delete(file_to_delete))?;
    }
    drop(operation_sender);

    println!("Performing {total_operations} sync operations");
    let (result_sender, result_receiver) = flume::unbounded();
    for _ in 0..std::thread::available_parallelism().unwrap().get() * 2 {
        let project = project.to_string();
        tokio::task::spawn(perform_sync_operations(
            operation_receiver.clone(),
            result_sender.clone(),
            project,
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

async fn list_files(
    remote_path: &str,
    database: &AnyDatabase<CliBackend>,
) -> anyhow::Result<HashMap<String, Bytes>> {
    match database {
        AnyDatabase::Local(database) => Ok(api::list_files(remote_path, database).await?),
        AnyDatabase::Networked(client) => Ok(client
            .storage()
            .send_api_request(&ListFiles {
                path: remote_path.to_string(),
            })
            .await?),
    }
}

#[derive(Debug)]
enum SyncOperation {
    Create(FileHash),
    Replace(FileHash),
    Delete(String),
}

#[derive(Debug)]
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

async fn perform_sync_operations(
    operations: flume::Receiver<SyncOperation>,
    result_sender: flume::Sender<anyhow::Result<String>>,
    project: String,
    database: AnyDatabase<CliBackend>,
) {
    while let Ok(op) = operations.recv_async().await {
        if result_sender
            .send(perform_sync_operation(op, &project, &database).await)
            .is_err()
        {
            break;
        }
    }
}

async fn perform_sync_operation(
    operation: SyncOperation,
    project: &str,
    database: &AnyDatabase<CliBackend>,
) -> anyhow::Result<String> {
    match operation {
        SyncOperation::Create(file_hash) => {
            upload_file(
                file_hash.path,
                file_hash.remote_path.clone(),
                project,
                database,
                Some(file_hash.blake3),
            )
            .await?;
            Ok(file_hash.remote_path)
        }
        SyncOperation::Replace(file_hash) => {
            upload_file(
                file_hash.path,
                file_hash.remote_path.clone(),
                project,
                database,
                Some(file_hash.blake3),
            )
            .await?;

            Ok(file_hash.remote_path)
        }
        SyncOperation::Delete(file) => {
            delete_file(&file, database).await?;

            Ok(file)
        }
    }
}

async fn delete_file(
    remote_path: &str,
    database: &AnyDatabase<CliBackend>,
) -> anyhow::Result<bool> {
    match database {
        AnyDatabase::Local(database) => Ok(api::delete_file(remote_path, database).await?),
        AnyDatabase::Networked(client) => Ok(client
            .storage()
            .send_api_request(&DeleteFile {
                path: remote_path.to_string(),
            })
            .await?),
    }
}

async fn write_file_data(
    path: &str,
    data: &[u8],
    start: bool,
    finished: bool,
    database: &AnyDatabase<CliBackend>,
) -> anyhow::Result<Option<Bytes>> {
    match database {
        AnyDatabase::Local(database) => {
            Ok(api::write_file_data(path, data, start, finished, database).await?)
        }
        AnyDatabase::Networked(client) => Ok(client
            .storage()
            .send_api_request(&WriteFileData {
                path: path.to_string(),
                data: Bytes::from(data),
                start,
                finished,
            })
            .await?),
    }
}

async fn backup(database: &AnyDatabase<CliBackend>, destination: &Path) -> anyhow::Result<()> {
    if !destination.exists() {
        std::fs::create_dir_all(destination)?;
    }

    let files = DossierFiles::list_recursive_async("/", database).await?;
    let mut tasks = Vec::new();
    let number_of_tasks = std::thread::available_parallelism().map_or(8, |t| t.get());
    let (sender, receiver) =
        flume::bounded::<File<Async<AnyDatabase<CliBackend>>, DossierFiles>>(number_of_tasks);

    for _ in 0..number_of_tasks {
        let receiver = receiver.clone();
        let folder = destination.to_path_buf();
        tasks.push(tokio::spawn(async move {
            let mut file_contents = Vec::new();
            while let Ok(file) = receiver.recv_async().await {
                let mut folder = folder.clone();
                for intermediate_name in file.containing_path().split_terminator('/').skip(1) {
                    folder.push(intermediate_name);
                }
                if !folder.exists() {
                    std::fs::create_dir_all(&folder)?;
                }

                let file_path = folder.join(file.name());
                if file_path.exists() {
                    // Check that the file hash doesn't match before re-downloading.
                    let mut hasher = blake3::Hasher::new();
                    let mut existing_file = fs::File::open(&file_path).await?;
                    let mut scratch = [0; 16 * 1024];
                    loop {
                        let bytes_read = existing_file.read(&mut scratch).await?;
                        if bytes_read > 0 {
                            hasher.update(&scratch[..bytes_read]);
                        } else {
                            break;
                        }
                    }
                    let hash = hasher.finalize().try_into().unwrap();
                    if file.metadata().map(|m| m.blake3) == Some(hash) {
                        println!("Skipping {}{}", file.containing_path(), file.name());
                        continue;
                    }
                }

                let mut contents = file.contents().await?;

                file_contents.clear();
                contents.read_to_end(&mut file_contents).await?;

                println!("Downloading {}{}", file.containing_path(), file.name());
                std::fs::write(file_path, &file_contents)?;
            }

            anyhow::Ok(())
        }));
    }

    for file in files {
        sender.send_async(file).await?;
    }

    drop(sender);

    for task in tasks {
        task.await??;
    }

    let projects = Project::all_async(database).await?;
    std::fs::write(
        destination.join("projects.ron"),
        ron::Options::default().to_string_pretty(&projects, PrettyConfig::default())?,
    )?;

    let api_tokens = ApiToken::all_async(database).await?;
    std::fs::write(
        destination.join("api-tokens.ron"),
        ron::Options::default().to_string_pretty(&api_tokens, PrettyConfig::default())?,
    )?;

    Ok(())
}
