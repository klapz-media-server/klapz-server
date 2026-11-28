//! Storage adapter for server settings, and the values libraries hold in place of them.

use klapz_core::settings::{
    SettingChange, SettingScope, SettingsError, SettingsRepository, StoredSetting,
};
use klapz_core::{SectionId, UserId};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter,
    TransactionTrait,
};

use crate::entity::{library_setting, server_setting};

#[derive(Debug, Clone)]
pub struct SqlSettingsRepository {
    connection: DatabaseConnection,
}

impl SqlSettingsRepository {
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl SettingsRepository for SqlSettingsRepository {
    async fn stored(
        &self,
        scope: SettingScope,
        keys: &[&str],
    ) -> Result<Vec<StoredSetting>, SettingsError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let keys = keys.iter().copied();
        Ok(match scope {
            SettingScope::Server => server_setting::Entity::find()
                .filter(server_setting::Column::Name.is_in(keys))
                .all(&self.connection)
                .await
                .map_err(storage)?
                .into_iter()
                .map(|row| stored(row.name, row.value, row.updated_at, row.updated_by))
                .collect(),
            SettingScope::Library(section) => library_setting::Entity::find()
                .filter(library_setting::Column::LibrarySectionId.eq(section.get()))
                .filter(library_setting::Column::Name.is_in(keys))
                .all(&self.connection)
                .await
                .map_err(storage)?
                .into_iter()
                .map(|row| stored(row.name, row.value, row.updated_at, row.updated_by))
                .collect(),
        })
    }

    async fn library_values(
        &self,
        keys: &[&str],
    ) -> Result<Vec<(SectionId, StoredSetting)>, SettingsError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        Ok(library_setting::Entity::find()
            .filter(library_setting::Column::Name.is_in(keys.iter().copied()))
            .all(&self.connection)
            .await
            .map_err(storage)?
            .into_iter()
            .map(|row| {
                (
                    SectionId::new(row.library_section_id),
                    stored(row.name, row.value, row.updated_at, row.updated_by),
                )
            })
            .collect())
    }

    async fn apply(
        &self,
        scope: SettingScope,
        changes: &[SettingChange],
        by: UserId,
        at: i64,
    ) -> Result<(), SettingsError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        for change in changes {
            match scope {
                SettingScope::Server => apply_to_server(&transaction, change, by, at).await,
                SettingScope::Library(section) => {
                    apply_to_library(&transaction, section, change, by, at).await
                }
            }
            .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }
}

fn stored(key: String, value: String, updated_at: i64, updated_by: Option<i64>) -> StoredSetting {
    StoredSetting {
        key,
        value,
        updated_at,
        updated_by: updated_by.map(UserId::new),
    }
}

async fn apply_to_server(
    transaction: &DatabaseTransaction,
    change: &SettingChange,
    by: UserId,
    at: i64,
) -> Result<(), sea_orm::DbErr> {
    match change {
        SettingChange::Set { key, value } => {
            server_setting::Entity::insert(server_setting::ActiveModel {
                name: ActiveValue::Set((*key).to_owned()),
                value: ActiveValue::Set(value.clone()),
                updated_at: ActiveValue::Set(at),
                updated_by: ActiveValue::Set(Some(by.get())),
            })
            .on_conflict(
                OnConflict::column(server_setting::Column::Name)
                    .update_columns([
                        server_setting::Column::Value,
                        server_setting::Column::UpdatedAt,
                        server_setting::Column::UpdatedBy,
                    ])
                    .to_owned(),
            )
            .exec(transaction)
            .await?;
        }
        SettingChange::Reset { key } => {
            server_setting::Entity::delete_by_id((*key).to_owned())
                .exec(transaction)
                .await?;
        }
    }
    Ok(())
}

async fn apply_to_library(
    transaction: &DatabaseTransaction,
    section: SectionId,
    change: &SettingChange,
    by: UserId,
    at: i64,
) -> Result<(), sea_orm::DbErr> {
    match change {
        SettingChange::Set { key, value } => {
            library_setting::Entity::insert(library_setting::ActiveModel {
                library_section_id: ActiveValue::Set(section.get()),
                name: ActiveValue::Set((*key).to_owned()),
                value: ActiveValue::Set(value.clone()),
                updated_at: ActiveValue::Set(at),
                updated_by: ActiveValue::Set(Some(by.get())),
            })
            .on_conflict(
                OnConflict::columns([
                    library_setting::Column::LibrarySectionId,
                    library_setting::Column::Name,
                ])
                .update_columns([
                    library_setting::Column::Value,
                    library_setting::Column::UpdatedAt,
                    library_setting::Column::UpdatedBy,
                ])
                .to_owned(),
            )
            .exec(transaction)
            .await?;
        }
        SettingChange::Reset { key } => {
            library_setting::Entity::delete_by_id((section.get(), (*key).to_owned()))
                .exec(transaction)
                .await?;
        }
    }
    Ok(())
}

fn storage(error: sea_orm::DbErr) -> SettingsError {
    SettingsError::Storage(Box::new(error))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

    use klapz_core::settings::{
        switches, update_library_settings, update_settings, value_of, SettingUpdate, DEEP_ANALYSIS,
        H264_PRESET,
    };
    use sea_orm::ConnectionTrait;

    use super::*;

    async fn repository() -> SqlSettingsRepository {
        SqlSettingsRepository::new(crate::connect("sqlite::memory:").await.unwrap())
    }

    fn update(value: Option<&str>) -> SettingUpdate {
        SettingUpdate {
            key: H264_PRESET.key.to_owned(),
            value: value.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn a_value_is_kept_replaced_and_forgotten() {
        let repository = repository().await;
        let (first, second) = (UserId::new(1), UserId::new(2));
        assert_eq!(
            value_of(&repository, &H264_PRESET).await.unwrap(),
            "veryfast"
        );

        update_settings(&repository, &[update(Some("slow"))], first, 10)
            .await
            .unwrap()
            .unwrap();
        update_settings(&repository, &[update(Some("medium"))], second, 20)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            repository
                .stored(SettingScope::Server, &[H264_PRESET.key])
                .await
                .unwrap(),
            [StoredSetting {
                key: H264_PRESET.key.to_owned(),
                value: "medium".to_owned(),
                updated_at: 20,
                updated_by: Some(second),
            }]
        );

        update_settings(&repository, &[update(None)], first, 30)
            .await
            .unwrap()
            .unwrap();
        assert!(repository
            .stored(SettingScope::Server, &[H264_PRESET.key])
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            value_of(&repository, &H264_PRESET).await.unwrap(),
            "veryfast"
        );
    }

    #[tokio::test]
    async fn a_librarys_value_is_its_own_and_goes_with_it() {
        let repository = repository().await;
        repository
            .connection
            .execute_unprepared(
                "INSERT INTO library_section (id, kind, title, language, uuid, created_at, \
                 updated_at, content_changed_at, hidden) \
                 VALUES (1, 'movie', 'Films', 'en', 'a', 0, 0, 0, false), \
                 (2, 'show', 'Series', 'en', 'b', 0, 0, 0, false)",
            )
            .await
            .unwrap();
        let owner = UserId::new(1);
        let off = SettingUpdate {
            key: DEEP_ANALYSIS.key.to_owned(),
            value: Some("false".to_owned()),
        };
        for section in [1, 2] {
            update_library_settings(
                &repository,
                SectionId::new(section),
                std::slice::from_ref(&off),
                owner,
                10,
            )
            .await
            .unwrap()
            .unwrap();
        }
        assert!(repository
            .stored(SettingScope::Server, &[DEEP_ANALYSIS.key])
            .await
            .unwrap()
            .is_empty());
        let deep = switches(&repository, &[&DEEP_ANALYSIS])
            .await
            .unwrap()
            .remove(0);
        assert!(deep.server);
        assert_eq!(deep.except, [SectionId::new(1), SectionId::new(2)]);

        repository
            .connection
            .execute_unprepared("DELETE FROM library_section WHERE id = 1")
            .await
            .unwrap();
        assert_eq!(
            switches(&repository, &[&DEEP_ANALYSIS]).await.unwrap()[0].except,
            [SectionId::new(2)]
        );
    }
}
