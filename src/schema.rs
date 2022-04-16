use bonsaidb::core::{
    connection::AsyncConnection,
    document::{CollectionDocument, Emit},
    schema::{Collection, NamedCollection, Schema, SerializedCollection},
};
use bonsaidb_files::{BonsaiFiles, FileConfig, FilesSchema};
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};

#[derive(Schema, Debug)]
#[schema(name = "dossier", collections = [Project, ApiToken], include = [FilesSchema<DossierFiles>])]
pub struct Dossier;

#[derive(Debug)]
pub enum DossierFiles {}

impl FileConfig for DossierFiles {
    type Metadata = Metadata;
    const BLOCK_SIZE: usize = BonsaiFiles::BLOCK_SIZE;

    fn files_name() -> bonsaidb::core::schema::CollectionName {
        BonsaiFiles::files_name()
    }

    fn blocks_name() -> bonsaidb::core::schema::CollectionName {
        BonsaiFiles::blocks_name()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub blake3: [u8; 32],
}

#[derive(Collection, Debug, Clone, Serialize, Deserialize)]
#[collection(name = "projects", primary_key = u32, views = [ProjectBySlug])]
pub struct Project {
    pub slug: String,
}

bonsaidb::core::define_basic_unique_mapped_view!(
    ProjectBySlug,
    Project,
    1,
    "by-slug",
    String,
    |project: CollectionDocument<Project>| project.header.emit_key(project.contents.slug)
);

impl NamedCollection for Project {
    type ByNameView = ProjectBySlug;
}

#[derive(Collection, Debug, Clone, Serialize, Deserialize)]
#[collection(name = "api-tokens", primary_key = u64)]
pub struct ApiToken {
    pub project_id: u32,
}

impl ApiToken {
    pub async fn create<C: AsyncConnection>(
        project_id: u32,
        connection: &C,
    ) -> anyhow::Result<CollectionDocument<Self>> {
        loop {
            let random_id = thread_rng().gen::<u64>();
            let result = ApiToken { project_id }
                .insert_into_async(random_id, connection)
                .await;
            match result {
                Ok(doc) => break Ok(doc),
                Err(err) if err.error.conflicting_document::<Self>().is_some() => {
                    // try again with a new random number
                }
                Err(other) => anyhow::bail!(other.error),
            }
        }
    }
}
