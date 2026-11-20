//! Stored users, permissions and extensions.

use klapz_core::access::{
    AccessError, AccessRepository, AccountLink, Extension, ExtensionDevice, Grant,
    LibraryPermission, LibraryRole, Login, NewUser, RemoteAccount, ServerRole, User, UserExtension,
};
use klapz_core::page::{Page, Window};
use klapz_core::{SectionId, UserId};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
    QuerySelect, SqlErr, TransactionTrait,
};

use crate::entity::{
    extension_account, extension_device, library_permission, subtitle_offset, track_choice,
    user_extension, user_password, user_rating, users, viewing, viewing_history, watch_state,
};

#[derive(Debug, Clone)]
pub struct SqlAccessRepository {
    connection: DatabaseConnection,
}

impl SqlAccessRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    async fn connection(
        &self,
        user: UserId,
        extension: Extension,
    ) -> Result<Option<UserExtension>, AccessError> {
        Ok(user_extension::Entity::find()
            .filter(user_extension::Column::UserId.eq(user.get()))
            .filter(user_extension::Column::Extension.eq(extension_name(extension)))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .and_then(user_extension_of))
    }

    /// What a connection opens, once it is known to be switched on: nothing when its user is
    /// not active.
    async fn grant_of(&self, extension: UserExtension) -> Result<Option<Grant>, AccessError> {
        let Some(user) = self.user(extension.user).await?.filter(|user| user.active) else {
            return Ok(None);
        };
        let permissions = self.permissions(user.id).await?;
        Ok(Some(Grant {
            user,
            extension,
            permissions,
        }))
    }

    /// Whether one of the two unique columns already holds the value: checked before the
    /// insert, because a violated constraint does not say portably which one it was.
    async fn taken(
        connection: &impl sea_orm::ConnectionTrait,
        column: users::Column,
        value: &str,
    ) -> Result<bool, AccessError> {
        Ok(users::Entity::find()
            .filter(column.eq(value))
            .one(connection)
            .await
            .map_err(storage)?
            .is_some())
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

const fn server_role_name(role: ServerRole) -> &'static str {
    match role {
        ServerRole::User => "user",
        ServerRole::Owner => "owner",
    }
}

/// A role this version does not know grants nothing beyond a user's.
fn server_role_of(name: &str) -> ServerRole {
    if name == "owner" {
        ServerRole::Owner
    } else {
        ServerRole::User
    }
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
        Extension::Jellyfin => "jellyfin",
    }
}

fn extension_of(name: &str) -> Option<Extension> {
    match name {
        "plex" => Some(Extension::Plex),
        "jellyfin" => Some(Extension::Jellyfin),
        _ => None,
    }
}

fn user_of(row: users::Model) -> User {
    User {
        id: UserId::new(row.id),
        username: row.username,
        email: row.email,
        role: server_role_of(&row.role),
        active: row.active,
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
        enabled: row.enabled,
    })
}

fn account_link_of(row: extension_account::Model, connection: UserExtension) -> AccountLink {
    AccountLink {
        user: connection.user,
        extension: connection.extension,
        server_id: connection.external_id,
        enabled: connection.enabled,
        account: RemoteAccount {
            id: row.account_id,
            name: row.account_name,
            token: row.account_token,
        },
        connection_uri: row.connection_uri,
        published_at: row.published_at,
    }
}

#[async_trait::async_trait]
impl AccessRepository for SqlAccessRepository {
    async fn create_user(&self, new: NewUser<'_>, now: i64) -> Result<User, AccessError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        if Self::taken(&transaction, users::Column::Username, new.username).await? {
            return Err(AccessError::UsernameTaken);
        }
        if let Some(email) = new.email {
            if Self::taken(&transaction, users::Column::Email, email).await? {
                return Err(AccessError::EmailTaken);
            }
        }
        let row = users::ActiveModel {
            id: ActiveValue::NotSet,
            username: ActiveValue::Set(new.username.to_owned()),
            email: ActiveValue::Set(new.email.map(str::to_owned)),
            role: ActiveValue::Set(server_role_name(new.role).to_owned()),
            active: ActiveValue::Set(new.active),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };
        let user = users::Entity::insert(row)
            .exec_with_returning(&transaction)
            .await
            .map_err(|error| match error.sql_err() {
                // Two creations racing past the checks above.
                Some(SqlErr::UniqueConstraintViolation(_)) => AccessError::UsernameTaken,
                _ => storage(error),
            })?;
        if let Some(hash) = new.password_hash {
            user_password::Entity::insert(user_password::ActiveModel {
                user_id: ActiveValue::Set(user.id),
                password_hash: ActiveValue::Set(hash.to_owned()),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            })
            .exec(&transaction)
            .await
            .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(user_of(user))
    }

    async fn credentials(&self, login: &Login) -> Result<Option<(User, String)>, AccessError> {
        let by = match login {
            Login::Username(username) => users::Column::Username.eq(username.as_str()),
            Login::Email(email) => users::Column::Email.eq(email.as_str()),
        };
        let Some(user) = users::Entity::find()
            .filter(by)
            .one(&self.connection)
            .await
            .map_err(storage)?
        else {
            return Ok(None);
        };
        Ok(user_password::Entity::find_by_id(user.id)
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(|password| (user_of(user), password.password_hash)))
    }

    async fn user_named(&self, username: &str) -> Result<Option<User>, AccessError> {
        Ok(users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(user_of))
    }

    async fn has_password(&self, user: UserId) -> Result<bool, AccessError> {
        Ok(user_password::Entity::find_by_id(user.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .is_some())
    }

    async fn set_password(
        &self,
        user: UserId,
        password_hash: &str,
        now: i64,
    ) -> Result<(), AccessError> {
        self.require_user(user).await?;
        user_password::Entity::insert(user_password::ActiveModel {
            user_id: ActiveValue::Set(user.get()),
            password_hash: ActiveValue::Set(password_hash.to_owned()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        })
        .on_conflict(
            OnConflict::column(user_password::Column::UserId)
                .update_columns([
                    user_password::Column::PasswordHash,
                    user_password::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(&self.connection)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn set_role(
        &self,
        user: UserId,
        role: ServerRole,
        now: i64,
    ) -> Result<Option<User>, AccessError> {
        users::Entity::update_many()
            .col_expr(users::Column::Role, server_role_name(role).into())
            .col_expr(users::Column::UpdatedAt, now.into())
            .filter(users::Column::Id.eq(user.get()))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        self.user(user).await
    }

    async fn set_active(
        &self,
        user: UserId,
        active: bool,
        now: i64,
    ) -> Result<Option<User>, AccessError> {
        users::Entity::update_many()
            .col_expr(users::Column::Active, active.into())
            .col_expr(users::Column::UpdatedAt, now.into())
            .filter(users::Column::Id.eq(user.get()))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        self.user(user).await
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
        extension_device::Entity::delete_many()
            .filter(extension_device::Column::UserId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        extension_account::Entity::delete_many()
            .filter(extension_account::Column::UserId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
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
        user_password::Entity::delete_by_id(id.get())
            .exec(&transaction)
            .await
            .map_err(storage)?;
        // No foreign key reaches these: they were keyed before every viewer had a row.
        watch_state::Entity::delete_many()
            .filter(watch_state::Column::Viewer.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        viewing::Entity::delete_many()
            .filter(viewing::Column::Viewer.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        viewing_history::Entity::delete_many()
            .filter(viewing_history::Column::Viewer.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        track_choice::Entity::delete_many()
            .filter(track_choice::Column::Viewer.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        user_rating::Entity::delete_many()
            .filter(user_rating::Column::Viewer.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        subtitle_offset::Entity::delete_many()
            .filter(subtitle_offset::Column::Viewer.eq(id.get()))
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
            enabled: ActiveValue::Set(true),
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
                    user_extension::Column::Enabled,
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

    async fn disable_extension(
        &self,
        user: UserId,
        extension: Extension,
        now: i64,
    ) -> Result<bool, AccessError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        extension_device::Entity::delete_many()
            .filter(extension_device::Column::UserId.eq(user.get()))
            .filter(extension_device::Column::Extension.eq(extension_name(extension)))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        let result = user_extension::Entity::update_many()
            .col_expr(user_extension::Column::Enabled, false.into())
            .col_expr(user_extension::Column::UpdatedAt, now.into())
            .filter(user_extension::Column::UserId.eq(user.get()))
            .filter(user_extension::Column::Extension.eq(extension_name(extension)))
            .filter(user_extension::Column::Enabled.eq(true))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(result.rows_affected > 0)
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
        let connection = user_extension::Entity::find()
            .filter(user_extension::Column::Extension.eq(extension_name(extension)))
            .filter(user_extension::Column::TokenDigest.eq(token_digest))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .and_then(user_extension_of);
        let connection = match connection {
            Some(connection) => Some(connection),
            None => match extension_device::Entity::find()
                .filter(extension_device::Column::Extension.eq(extension_name(extension)))
                .filter(extension_device::Column::TokenDigest.eq(token_digest))
                .one(&self.connection)
                .await
                .map_err(storage)?
            {
                Some(device) => {
                    self.connection(UserId::new(device.user_id), extension)
                        .await?
                }
                None => None,
            },
        };
        match connection.filter(|connection| connection.enabled) {
            Some(connection) => self.grant_of(connection).await,
            None => Ok(None),
        }
    }

    async fn issue_device_token(
        &self,
        user: UserId,
        extension: Extension,
        device: &ExtensionDevice,
        token_digest: &str,
        now: i64,
    ) -> Result<(), AccessError> {
        let connected = self
            .connection(user, extension)
            .await?
            .is_some_and(|connection| connection.enabled);
        if !connected {
            return Err(AccessError::NotConnected);
        }
        let row = extension_device::ActiveModel {
            id: ActiveValue::NotSet,
            user_id: ActiveValue::Set(user.get()),
            extension: ActiveValue::Set(extension_name(extension).to_owned()),
            device_id: ActiveValue::Set(device.id.clone()),
            device_name: ActiveValue::Set(device.name.clone()),
            client: ActiveValue::Set(device.client.clone()),
            client_version: ActiveValue::Set(device.client_version.clone()),
            token_digest: ActiveValue::Set(token_digest.to_owned()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };
        extension_device::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([
                    extension_device::Column::UserId,
                    extension_device::Column::Extension,
                    extension_device::Column::DeviceId,
                ])
                .update_columns([
                    extension_device::Column::DeviceName,
                    extension_device::Column::Client,
                    extension_device::Column::ClientVersion,
                    extension_device::Column::TokenDigest,
                    extension_device::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn revoke_device_token(
        &self,
        user: UserId,
        extension: Extension,
        token_digest: &str,
    ) -> Result<bool, AccessError> {
        let result = extension_device::Entity::delete_many()
            .filter(extension_device::Column::UserId.eq(user.get()))
            .filter(extension_device::Column::Extension.eq(extension_name(extension)))
            .filter(extension_device::Column::TokenDigest.eq(token_digest))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn link_account(
        &self,
        user: UserId,
        extension: Extension,
        account: &RemoteAccount,
        connection_uri: &str,
        now: i64,
    ) -> Result<AccountLink, AccessError> {
        let connection = self
            .connection(user, extension)
            .await?
            .ok_or(AccessError::NotConnected)?;
        let row = extension_account::ActiveModel {
            id: ActiveValue::NotSet,
            user_id: ActiveValue::Set(user.get()),
            extension: ActiveValue::Set(extension_name(extension).to_owned()),
            account_id: ActiveValue::Set(account.id.clone()),
            account_name: ActiveValue::Set(account.name.clone()),
            account_token: ActiveValue::Set(account.token.clone()),
            connection_uri: ActiveValue::Set(connection_uri.to_owned()),
            published_at: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };
        extension_account::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([
                    extension_account::Column::UserId,
                    extension_account::Column::Extension,
                ])
                .update_columns([
                    extension_account::Column::AccountId,
                    extension_account::Column::AccountName,
                    extension_account::Column::AccountToken,
                    extension_account::Column::ConnectionUri,
                    extension_account::Column::PublishedAt,
                    extension_account::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(|error| match error.sql_err() {
                Some(SqlErr::UniqueConstraintViolation(_)) => AccessError::AccountTaken,
                _ => storage(error),
            })?;
        Ok(AccountLink {
            user,
            extension,
            server_id: connection.external_id,
            enabled: connection.enabled,
            account: account.clone(),
            connection_uri: connection_uri.to_owned(),
            published_at: None,
        })
    }

    async fn account_link(
        &self,
        user: UserId,
        extension: Extension,
    ) -> Result<Option<AccountLink>, AccessError> {
        let row = extension_account::Entity::find()
            .filter(extension_account::Column::UserId.eq(user.get()))
            .filter(extension_account::Column::Extension.eq(extension_name(extension)))
            .one(&self.connection)
            .await
            .map_err(storage)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(self
            .connection(user, extension)
            .await?
            .map(|connection| account_link_of(row, connection)))
    }

    async fn connection_at(
        &self,
        extension: Extension,
        connection_uris: &[String],
    ) -> Result<Option<UserExtension>, AccessError> {
        let rows = extension_account::Entity::find()
            .filter(extension_account::Column::Extension.eq(extension_name(extension)))
            .filter(extension_account::Column::ConnectionUri.is_in(connection_uris.iter().cloned()))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut found = None;
        for row in rows {
            let connection = self.connection(UserId::new(row.user_id), extension).await?;
            if let Some(connection) = connection.filter(|connection| connection.enabled) {
                if found.is_some() {
                    return Ok(None);
                }
                found = Some(connection);
            }
        }
        Ok(found)
    }

    async fn unlink_account(
        &self,
        user: UserId,
        extension: Extension,
    ) -> Result<bool, AccessError> {
        let result = extension_account::Entity::delete_many()
            .filter(extension_account::Column::UserId.eq(user.get()))
            .filter(extension_account::Column::Extension.eq(extension_name(extension)))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn move_account(
        &self,
        user: UserId,
        extension: Extension,
        connection_uri: &str,
        now: i64,
    ) -> Result<Option<AccountLink>, AccessError> {
        extension_account::Entity::update_many()
            .col_expr(
                extension_account::Column::ConnectionUri,
                connection_uri.to_owned().into(),
            )
            .col_expr(
                extension_account::Column::PublishedAt,
                Option::<i64>::None.into(),
            )
            .col_expr(extension_account::Column::UpdatedAt, now.into())
            .filter(extension_account::Column::UserId.eq(user.get()))
            .filter(extension_account::Column::Extension.eq(extension_name(extension)))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        self.account_link(user, extension).await
    }

    async fn mark_published(
        &self,
        user: UserId,
        extension: Extension,
        at: i64,
    ) -> Result<(), AccessError> {
        extension_account::Entity::update_many()
            .col_expr(extension_account::Column::PublishedAt, at.into())
            .col_expr(extension_account::Column::UpdatedAt, at.into())
            .filter(extension_account::Column::UserId.eq(user.get()))
            .filter(extension_account::Column::Extension.eq(extension_name(extension)))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn grant_for_account(
        &self,
        extension: Extension,
        account_id: &str,
    ) -> Result<Option<Grant>, AccessError> {
        let row = extension_account::Entity::find()
            .filter(extension_account::Column::Extension.eq(extension_name(extension)))
            .filter(extension_account::Column::AccountId.eq(account_id))
            .one(&self.connection)
            .await
            .map_err(storage)?;
        let Some(row) = row else {
            return Ok(None);
        };
        match self.connection(UserId::new(row.user_id), extension).await? {
            Some(connection) if connection.enabled => self.grant_of(connection).await,
            _ => Ok(None),
        }
    }
}
