use bonsaidb::core::{
    admin::{AuthenticationToken, PermissionGroup, Role},
    connection::{AsyncConnection, IdentityReference},
    document::{CollectionDocument, Emit},
    permissions::Statement,
    schema::{Collection, NamedCollection, Schema, SerializedCollection},
};
use bonsaidb_files::{BonsaiFiles, FileConfig, FilesSchema};
use serde::{Deserialize, Serialize};

use crate::permissions::{project_resource_name, DossierAction};

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
#[collection(name = "api-tokens", primary_key = u64, natural_id = |token: &ApiToken| Some(token.authentication_token_id))]
pub struct ApiToken {
    pub label: String,
    pub authentication_token_id: u64,
    pub project_id: u32,
}

impl ApiToken {
    pub async fn create<C: AsyncConnection>(
        label: String,
        project_id: u32,
        connection: &C,
        admin: &C,
    ) -> anyhow::Result<(
        CollectionDocument<Self>,
        CollectionDocument<AuthenticationToken>,
    )> {
        let group = PermissionGroup {
            name: label.clone(),
            statements: vec![Statement::for_resource(project_resource_name(project_id))
                .allowing(&DossierAction::SyncFiles)],
        }
        .push_into_async(admin)
        .await?;
        let role = Role {
            name: label.clone(),
            groups: vec![group.header.id],
        }
        .push_into_async(admin)
        .await?;
        let authentication_token =
            AuthenticationToken::create_async(IdentityReference::role(role.header.id)?, admin)
                .await?;
        let api_token = ApiToken {
            project_id,
            label,
            authentication_token_id: authentication_token.header.id,
        }
        .push_into_async(connection)
        .await?;
        Ok((api_token, authentication_token))
    }

    pub async fn delete<C: AsyncConnection>(
        api_token: &CollectionDocument<Self>,
        connection: &C,
        admin: &C,
    ) -> anyhow::Result<()> {
        if let Some(auth_token) =
            AuthenticationToken::get_async(&api_token.header.id, admin).await?
        {
            auth_token.delete_async(admin).await?;
            println!("Authentication Token {} deleted", auth_token.header.id);
        }
        if let Some(role) = Role::load_async(&api_token.contents.label, admin).await? {
            role.delete_async(admin).await?;
            println!("Role {} deleted", role.header.id);
        }
        if let Some(group) = PermissionGroup::load_async(&api_token.contents.label, admin).await? {
            group.delete_async(admin).await?;
            println!("Permission Group {} deleted", group.header.id);
        }

        api_token.delete_async(connection).await?;
        println!("Api Token {} deleted", api_token.contents.label);
        Ok(())
    }
}
