//! Stored users, permissions and extensions.

use open_plex_core::access::{
    AccessError, AccessRepository, Extension, Grant, LibraryPermission, LibraryRole, User,
    UserExtension,
};
use open_plex_core::page::{Page, Window};
use open_plex_core::{SectionId, UserId};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
    QuerySelect, SqlErr, TransactionTrait,
};

use crate::entity::{library_permission, user_extension, users};

#[derive(Debug, Clone)]
pub struct SqlAccessRepository {
    connection: DatabaseConnection,
}

impl SqlAccessRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    async fn require_user(&self, id: UserId) -> Result<(), AccessError> {
        if self.user(id).await?.is_some() {
            Ok(())
        } else {
            Err(AccessError::UnknownUser)
        }
    }
}

fn storage(error: sea_orm::DbErr) -> AccessError {
    AccessError::Storage(Box::new(error))
}

const fn role_name(role: LibraryRole) -> &'static str {
    match role {
        LibraryRole::Viewer => "viewer",
        LibraryRole::Admin => "admin",
    }
}

/// A role this version does not know grants nothing beyond viewing.
fn role_of(name: &str) -> LibraryRole {
    if name == "admin" {
        LibraryRole::Admin
    } else {
        LibraryRole::Viewer
    }
}

const fn extension_name(extension: Extension) -> &'static str {
    match extension {
        Extension::Plex => "plex",
    }
}

fn extension_of(name: &str) -> Option<Extension> {
    (name == "plex").then_some(Extension::Plex)
}

fn user_of(row: users::Model) -> User {
    User {
        id: UserId::new(row.id),
        email: row.email,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn permission_of(row: &library_permission::Model) -> LibraryPermission {
    LibraryPermission {
        user: UserId::new(row.user_id),
        section: SectionId::new(row.section_id),
        role: role_of(&row.role),
    }
}

fn user_extension_of(row: user_extension::Model) -> Option<UserExtension> {
    Some(UserExtension {
        user: UserId::new(row.user_id),
        extension: extension_of(&row.extension)?,
        external_id: row.external_id,
    })
}

#[async_trait::async_trait]
impl AccessRepository for SqlAccessRepository {
    async fn create_user(&self, email: &str, now: i64) -> Result<User, AccessError> {
        let row = users::ActiveModel {
            id: ActiveValue::NotSet,
            email: ActiveValue::Set(email.to_owned()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };
        let inserted = users::Entity::insert(row)
            .exec_with_returning(&self.connection)
            .await
            .map_err(|error| match error.sql_err() {
                Some(SqlErr::UniqueConstraintViolation(_)) => AccessError::EmailTaken,
                _ => storage(error),
            })?;
        Ok(user_of(inserted))
    }

    async fn users(&self, window: Window) -> Result<Page<User>, AccessError> {
        let mut select = users::Entity::find();
        if let Some(after) = window.after.as_ref() {
            select = select.filter(users::Column::Id.gt(after.id));
        }
        if let Some(before) = window.before.as_ref() {
            select = select.filter(users::Column::Id.lt(before.id));
        }
        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(users::Column::Id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut page = Page::from_overfetch(rows, &window).map(user_of);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn user(&self, id: UserId) -> Result<Option<User>, AccessError> {
        Ok(users::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(user_of))
    }

    async fn delete_user(&self, id: UserId) -> Result<bool, AccessError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        user_extension::Entity::delete_many()
            .filter(user_extension::Column::UserId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        library_permission::Entity::delete_many()
            .filter(library_permission::Column::UserId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        let deleted = users::Entity::delete_by_id(id.get())
            .exec(&transaction)
            .await
            .map_err(storage)?
            .rows_affected;
        transaction.commit().await.map_err(storage)?;
        Ok(deleted > 0)
    }

    async fn set_permission(
        &self,
        permission: LibraryPermission,
        now: i64,
    ) -> Result<(), AccessError> {
        self.require_user(permission.user).await?;
        let row = library_permission::ActiveModel {
            id: ActiveValue::NotSet,
            user_id: ActiveValue::Set(permission.user.get()),
            section_id: ActiveValue::Set(permission.section.get()),
            role: ActiveValue::Set(role_name(permission.role).to_owned()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };
        library_permission::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([
                    library_permission::Column::UserId,
                    library_permission::Column::SectionId,
                ])
                .update_columns([
                    library_permission::Column::Role,
                    library_permission::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn remove_permission(
        &self,
        user: UserId,
        section: SectionId,
    ) -> Result<bool, AccessError> {
        let result = library_permission::Entity::delete_many()
            .filter(library_permission::Column::UserId.eq(user.get()))
            .filter(library_permission::Column::SectionId.eq(section.get()))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn permissions(&self, user: UserId) -> Result<Vec<LibraryPermission>, AccessError> {
        Ok(library_permission::Entity::find()
            .filter(library_permission::Column::UserId.eq(user.get()))
            .order_by_asc(library_permission::Column::SectionId)
            .all(&self.connection)
            .await
            .map_err(storage)?
            .iter()
            .map(permission_of)
            .collect())
    }

    async fn connect_extension(
        &self,
        user: UserId,
        extension: Extension,
        external_id: &str,
        token_digest: &str,
        now: i64,
    ) -> Result<UserExtension, AccessError> {
        self.require_user(user).await?;
        let row = user_extension::ActiveModel {
            id: ActiveValue::NotSet,
            user_id: ActiveValue::Set(user.get()),
            extension: ActiveValue::Set(extension_name(extension).to_owned()),
            external_id: ActiveValue::Set(external_id.to_owned()),
            token_digest: ActiveValue::Set(token_digest.to_owned()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };
        user_extension::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([
                    user_extension::Column::UserId,
                    user_extension::Column::Extension,
                ])
                .update_columns([
                    user_extension::Column::TokenDigest,
                    user_extension::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        let stored = user_extension::Entity::find()
            .filter(user_extension::Column::UserId.eq(user.get()))
            .filter(user_extension::Column::Extension.eq(extension_name(extension)))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .ok_or(AccessError::UnknownUser)?;
        user_extension_of(stored).ok_or(AccessError::UnknownUser)
    }

    async fn extensions(&self, user: UserId) -> Result<Vec<UserExtension>, AccessError> {
        Ok(user_extension::Entity::find()
            .filter(user_extension::Column::UserId.eq(user.get()))
            .all(&self.connection)
            .await
            .map_err(storage)?
            .into_iter()
            .filter_map(user_extension_of)
            .collect())
    }

    async fn grant(
        &self,
        extension: Extension,
        token_digest: &str,
    ) -> Result<Option<Grant>, AccessError> {
        let Some(row) = user_extension::Entity::find()
            .filter(user_extension::Column::Extension.eq(extension_name(extension)))
            .filter(user_extension::Column::TokenDigest.eq(token_digest))
            .one(&self.connection)
            .await
            .map_err(storage)?
        else {
            return Ok(None);
        };
        let Some(extension) = user_extension_of(row) else {
            return Ok(None);
        };
        let Some(user) = self.user(extension.user).await? else {
            return Ok(None);
        };
        let permissions = self.permissions(user.id).await?;
        Ok(Some(Grant {
            user,
            extension,
            permissions,
        }))
    }
}
