use std::{collections::HashMap, future::Future};

use bonsaidb::{
    core::{
        api::Api,
        arc_bytes::serde::Bytes,
        async_trait::async_trait,
        connection::{AsyncConnection, AsyncStorageConnection, HasSession},
        schema::NamedCollection,
    },
    server::{
        api::{Handler, HandlerError, HandlerResult, HandlerSession},
        ServerDatabase,
    },
};
use bonsaidb_files::{FileConfig, Truncate};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::{
    permissions::{project_resource_name, DossierAction},
    schema::{Dossier, DossierFiles, Metadata, Project},
    CliBackend,
};

#[derive(Debug)]
pub struct DossierApiHandler;

#[derive(thiserror::Error, Debug, Serialize, Deserialize, Clone)]
pub enum ApiError {
    #[error("project not found")]
    ProjectNotFound,
    /// A name contained an invalid character. Currently, the only disallowed
    /// character is `/`.
    #[error("names must not contain '/'")]
    InvalidName,
    /// An absolute path was expected, but the path provided did not include a
    /// leading `/`.
    #[error("all paths must start with a leading '/'")]
    InvalidPath,
    /// An attempt at creating a file failed because a file already existed.
    #[error("a file already exists at the path provided")]
    AlreadyExists,
    /// The file was deleted during the operation.
    #[error("the file was deleted during the operation")]
    Deleted,
}

trait ResultExt<T> {
    fn map_files_error(self) -> Result<T, HandlerError<ApiError>>;
}

impl<A> ResultExt<A> for Result<A, bonsaidb_files::Error> {
    fn map_files_error(self) -> Result<A, HandlerError<ApiError>> {
        match self {
            Ok(result) => Ok(result),
            Err(bonsaidb_files::Error::Database(db)) => {
                Err(HandlerError::Server(bonsaidb::server::Error::from(db)))
            }
            Err(bonsaidb_files::Error::InvalidName) => {
                Err(HandlerError::Api(ApiError::InvalidName))
            }
            Err(bonsaidb_files::Error::InvalidPath) => {
                Err(HandlerError::Api(ApiError::InvalidPath))
            }
            Err(bonsaidb_files::Error::AlreadyExists) => {
                Err(HandlerError::Api(ApiError::AlreadyExists))
            }
            Err(bonsaidb_files::Error::Deleted) => Err(HandlerError::Api(ApiError::Deleted)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Api)]
#[api(name = "compute-changes", response = HashMap<String, Bytes>, error = ApiError)]
pub struct ListFiles {
    pub path: String,
}

#[async_trait]
impl Handler<CliBackend, ListFiles> for DossierApiHandler {
    async fn handle(
        session: HandlerSession<'_, CliBackend>,
        request: ListFiles,
    ) -> HandlerResult<ListFiles> {
        handle_sync_op_with_permissions(
            session,
            &request.path,
            &request,
            |database, request| async move { list_files(&request.path, &database).await },
        )
        .await
    }
}

pub async fn list_files<C: AsyncConnection + Clone>(
    base_path: &str,
    database: &C,
) -> HandlerResult<ListFiles> {
    Ok(DossierFiles::list_recursive_async(base_path, database)
        .await?
        .into_iter()
        .filter_map(|file| {
            file.metadata()
                .map(|metadata| (file.path(), Bytes::from(metadata.blake3.to_vec())))
        })
        .collect())
}

#[derive(Serialize, Deserialize, Debug, Api)]
#[api(name = "delete-file", response = bool, error = ApiError)]
pub struct DeleteFile {
    pub path: String,
}

#[async_trait]
impl Handler<CliBackend, DeleteFile> for DossierApiHandler {
    async fn handle(
        session: HandlerSession<'_, CliBackend>,
        request: DeleteFile,
    ) -> HandlerResult<DeleteFile> {
        handle_sync_op_with_permissions(
            session,
            &request.path,
            &request,
            |database, request| async move { delete_file(&request.path, &database).await },
        )
        .await
    }
}

pub async fn delete_file<C: AsyncConnection + Clone>(
    path: &str,
    database: &C,
) -> HandlerResult<DeleteFile> {
    DossierFiles::delete_async(path, database)
        .await
        .map_files_error()
}

#[derive(Serialize, Deserialize, Debug, Api)]
#[api(name = "write-file", response = Option<Bytes>, error = ApiError)]
pub struct WriteFileData {
    pub path: String,
    pub data: Bytes,
    pub start: bool,
    pub finished: bool,
}

#[async_trait]
impl Handler<CliBackend, WriteFileData> for DossierApiHandler {
    async fn handle(
        session: HandlerSession<'_, CliBackend>,
        request: WriteFileData,
    ) -> HandlerResult<WriteFileData> {
        handle_sync_op_with_permissions(
            session,
            &request.path,
            &request,
            |database, request| async move {
                write_file_data(
                    &request.path,
                    &request.data,
                    request.start,
                    request.finished,
                    &database,
                )
                .await
            },
        )
        .await
    }
}

pub async fn write_file_data<C: AsyncConnection + Clone + Unpin + 'static>(
    path: &str,
    data: &[u8],
    start: bool,
    finished: bool,
    database: &C,
) -> HandlerResult<WriteFileData> {
    let mut file = match DossierFiles::load_async(path, database)
        .await
        .map_files_error()?
    {
        Some(file) if start => {
            file.truncate(0, Truncate::RemovingStart).await?;
            file
        }
        Some(file) => file,
        None if start => DossierFiles::build(path)
            .create_async(database)
            .await
            .map_files_error()?,
        None => return Err(HandlerError::Api(ApiError::Deleted)),
    };

    file.append(data).await?;

    if finished {
        // Compute the hash of the file
        let mut contents = file.contents().await?;
        let mut sha = blake3::Hasher::new();
        while let Some(block) = contents.next().await {
            let block = block?;
            sha.update(&block);
        }

        let hash = sha.finalize().try_into().unwrap();
        file.update_metadata(Metadata { blake3: hash }).await?;

        Ok(Some(Bytes::from(hash.to_vec())))
    } else {
        Ok(None)
    }
}

async fn handle_sync_op_with_permissions<
    'future,
    A: Api<Error = ApiError>,
    Handle: FnOnce(ServerDatabase<CliBackend>, &'future A) -> F,
    F: Future<Output = HandlerResult<A>> + 'future,
>(
    session: HandlerSession<'_, CliBackend>,
    path: &str,
    request: &'future A,
    handler: Handle,
) -> HandlerResult<A> {
    let database = session.as_client.database::<Dossier>("dossier").await?;
    let project = path.split('/').nth(1);
    if let Some(project) = project {
        if let Some(project) = Project::load_async(project, &database).await? {
            session.as_client.check_permission(
                project_resource_name(project.header.id),
                &DossierAction::SyncFiles,
            )?;

            return handler(database, request).await;
        }
    }
    Err(HandlerError::Api(ApiError::ProjectNotFound))
}
