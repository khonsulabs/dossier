use bonsaidb::core::{
    admin::PermissionGroup,
    connection::AsyncStorageConnection,
    permissions::{Action, ResourceName, Statement},
    schema::NamedCollection,
};

pub async fn initialize<Storage: AsyncStorageConnection>(
    connection: &Storage,
) -> anyhow::Result<()> {
    let admin = connection.admin().await;

    PermissionGroup::entry_async("administrators", &admin)
        .or_insert_with(|| PermissionGroup {
            name: String::from("administrators"),
            statements: vec![Statement::allow_all_for_any_resource()],
        })
        .await?;

    Ok(())
}

pub fn project_resource_name(project_id: u32) -> ResourceName<'static> {
    ResourceName::named("dossier")
        .and("project")
        .and(u64::from(project_id))
}

#[derive(Action, Debug)]
#[action(actionable = bonsaidb::core::actionable)]
pub enum DossierAction {
    SyncFiles,
}
