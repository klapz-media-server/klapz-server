//! Storage adapter for server settings.

use klapz_core::settings::{SettingChange, SettingsError, SettingsRepository, StoredSetting};
use klapz_core::UserId;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};

use crate::entity::server_setting;

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
    async fn stored(&self, keys: &[&str]) -> Result<Vec<StoredSetting>, SettingsError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let rows = server_setting::Entity::find()
            .filter(server_setting::Column::Name.is_in(keys.iter().copied()))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows
            .into_iter()
            .map(|row| StoredSetting {
                key: row.name,
                value: row.value,
                updated_at: row.updated_at,
                updated_by: row.updated_by.map(UserId::new),
            })
            .collect())
    }

    async fn apply(
        &self,
        changes: &[SettingChange],
        by: UserId,
        at: i64,
    ) -> Result<(), SettingsError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        for change in changes {
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
                    .exec(&transaction)
                    .await
                    .map_err(storage)?;
                }
                SettingChange::Reset { key } => {
                    server_setting::Entity::delete_by_id((*key).to_owned())
                        .exec(&transaction)
                        .await
                        .map_err(storage)?;
                }
            }
        }
        transaction.commit().await.map_err(storage)
    }
}

fn storage(error: sea_orm::DbErr) -> SettingsError {
    SettingsError::Storage(Box::new(error))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

    use klapz_core::settings::{update_settings, value_of, SettingUpdate, H264_PRESET};

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
            repository.stored(&[H264_PRESET.key]).await.unwrap(),
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
            .stored(&[H264_PRESET.key])
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            value_of(&repository, &H264_PRESET).await.unwrap(),
            "veryfast"
        );
    }
}
