//! Every caller is a user: the operator, who had no row, is gone.
//!
//! Users gain a username, which they are known and sign in by, and a server role, `owner` or
//! `user`; the e-mail address becomes optional, since the built-in `admin` account has none.
//! Each existing user is named after its address's local part, made valid and given a suffix
//! when taken; `admin` is never handed out, as it belongs to the account the server creates at
//! startup.
//!
//! What was the operator's is deleted: its watch state, playbacks, history, track choices,
//! playlists and bookmarks (viewer 0), its play queues and playback sessions (no user). The
//! owners of the last two then become required. A bookmark's owner stays nullable: null is a
//! marker that belongs to the work rather than to a viewer.
//!
//! `SQLite` cannot change whether a column is nullable, so `users`, `play_queue` and
//! `playback_session` are rebuilt: a copy with the new definition, the rows carried over, the
//! old table dropped and the copy renamed, its indexes made again. No foreign key points at any
//! of the three.
//!
//! Down restores the old shapes, without what was deleted and without the users that have no
//! address, who cannot exist there.

use klapz_core::access::ADMIN_USERNAME;
use klapz_core::account::username_from_email;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        delete_the_operators(manager).await?;
        rebuild_users(manager, true).await?;
        rebuild(
            manager,
            PlayQueue::Table,
            play_queue(true),
            &PLAY_QUEUE_COLUMNS,
        )
        .await?;
        play_queue_indexes(manager).await?;
        rebuild(
            manager,
            PlaybackSession::Table,
            playback_session(true),
            &PLAYBACK_SESSION_COLUMNS,
        )
        .await?;
        playback_session_indexes(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild(
            manager,
            PlaybackSession::Table,
            playback_session(false),
            &PLAYBACK_SESSION_COLUMNS,
        )
        .await?;
        playback_session_indexes(manager).await?;
        rebuild(
            manager,
            PlayQueue::Table,
            play_queue(false),
            &PLAY_QUEUE_COLUMNS,
        )
        .await?;
        play_queue_indexes(manager).await?;
        rebuild_users(manager, false).await
    }
}

/// The operator watched, chose tracks and kept playlists as viewer 0, and queued and played
/// with no user at all.
async fn delete_the_operators(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for (table, column) in [
        (Keyed::WatchState, Keyed::Viewer),
        (Keyed::Viewing, Keyed::Viewer),
        (Keyed::ViewingHistory, Keyed::Viewer),
        (Keyed::TrackChoice, Keyed::Viewer),
        (Keyed::ItemMarker, Keyed::Owner),
    ] {
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(table)
                    .and_where(Expr::col(column).eq(0))
                    .to_owned(),
            )
            .await?;
    }
    let operators_playlists = Query::select()
        .column(Playlist::Id)
        .from(Playlist::Table)
        .and_where(Expr::col(Playlist::Owner).eq(0))
        .to_owned();
    manager
        .exec_stmt(
            Query::delete()
                .from_table(PlaylistEntry::Table)
                .and_where(Expr::col(PlaylistEntry::PlaylistId).in_subquery(operators_playlists))
                .to_owned(),
        )
        .await?;
    manager
        .exec_stmt(
            Query::delete()
                .from_table(Playlist::Table)
                .and_where(Expr::col(Playlist::Owner).eq(0))
                .to_owned(),
        )
        .await?;

    let operators_queues = Query::select()
        .column(PlayQueue::Id)
        .from(PlayQueue::Table)
        .and_where(Expr::col(PlayQueue::UserId).is_null())
        .to_owned();
    for table in [QueueContent::PlayQueueEntry, QueueContent::PlayQueueSource] {
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(table)
                    .and_where(
                        Expr::col(QueueContent::QueueId).in_subquery(operators_queues.clone()),
                    )
                    .to_owned(),
            )
            .await?;
    }
    for (table, column) in [
        (Owned::PlayQueue, Owned::UserId),
        (Owned::PlaybackSession, Owned::UserId),
    ] {
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(table)
                    .and_where(Expr::col(column).is_null())
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

/// Copies `table` into `next`, a table of the new shape named [`Next::Table`], and puts the copy
/// in its place. Its indexes went with the old table: the caller makes them again.
async fn rebuild(
    manager: &SchemaManager<'_>,
    table: impl Iden + Clone + 'static,
    next: TableCreateStatement,
    columns: &[&'static str],
) -> Result<(), DbErr> {
    manager.create_table(next).await?;
    let columns: Vec<Alias> = columns.iter().map(|column| Alias::new(*column)).collect();
    let copy = Query::insert()
        .into_table(Next::Table)
        .columns(columns.clone())
        .select_from(
            Query::select()
                .columns(columns)
                .from(table.clone())
                .to_owned(),
        )
        .map_err(|error| DbErr::Migration(error.to_string()))?
        .to_owned();
    manager.exec_stmt(copy).await?;
    replace(manager, table).await
}

async fn replace(
    manager: &SchemaManager<'_>,
    table: impl Iden + Clone + 'static,
) -> Result<(), DbErr> {
    manager
        .drop_table(Table::drop().table(table.clone()).to_owned())
        .await?;
    manager
        .rename_table(Table::rename().table(Next::Table, table.clone()).to_owned())
        .await?;
    // Rows were copied with their ids, which moves `SQLite`'s and `MySQL`'s counters but not a
    // `PostgreSQL` sequence: the next insert would reuse an id.
    if manager.get_database_backend() == DbBackend::Postgres {
        let name = table.to_string();
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "SELECT setval(pg_get_serial_sequence('{name}', 'id'), \
                 COALESCE((SELECT MAX(id) FROM \"{name}\"), 0) + 1, false)"
            ))
            .await?;
    }
    Ok(())
}

/// `users` with a username and a role, an optional address when `current`; as it was before
/// otherwise.
fn users_table(current: bool) -> TableCreateStatement {
    let mut next = Table::create();
    next.table(Next::Table).col(
        ColumnDef::new(Users::Id)
            .big_integer()
            .not_null()
            .auto_increment()
            .primary_key(),
    );
    if current {
        next.col(ColumnDef::new(Users::Username).string().not_null())
            .col(ColumnDef::new(Users::Email).string())
            .col(
                ColumnDef::new(Users::Role)
                    .string()
                    .not_null()
                    .default("user"),
            );
    } else {
        next.col(ColumnDef::new(Users::Email).string().not_null());
    }
    next.col(ColumnDef::new(Users::CreatedAt).big_integer().not_null())
        .col(ColumnDef::new(Users::UpdatedAt).big_integer().not_null())
        .col(
            ColumnDef::new(Users::Active)
                .boolean()
                .not_null()
                .default(true),
        )
        .to_owned()
}

/// Rebuilds `users` as [`users_table`] has it, each user named when `current`, and a user with
/// no address left out otherwise.
async fn rebuild_users(manager: &SchemaManager<'_>, current: bool) -> Result<(), DbErr> {
    manager.create_table(users_table(current)).await?;
    let backend = manager.get_database_backend();
    let rows = manager
        .get_connection()
        .query_all(
            backend.build(
                Query::select()
                    .columns([
                        Users::Id,
                        Users::Email,
                        Users::Active,
                        Users::CreatedAt,
                        Users::UpdatedAt,
                    ])
                    .from(Users::Table)
                    .order_by(Users::Id, Order::Asc),
            ),
        )
        .await?;
    let mut taken = vec![ADMIN_USERNAME.to_owned()];
    for row in rows {
        let email: Option<String> = row.try_get_by_index(1)?;
        let mut columns = vec![
            Users::Id,
            Users::Email,
            Users::Active,
            Users::CreatedAt,
            Users::UpdatedAt,
        ];
        let mut values: Vec<SimpleExpr> = vec![
            row.try_get_by_index::<i64>(0)?.into(),
            email.clone().into(),
            row.try_get_by_index::<bool>(2)?.into(),
            row.try_get_by_index::<i64>(3)?.into(),
            row.try_get_by_index::<i64>(4)?.into(),
        ];
        if current {
            let username = unique_username(email.as_deref().unwrap_or_default(), &taken);
            taken.push(username.clone());
            columns.push(Users::Username);
            values.push(username.into());
        } else if email.is_none() {
            continue;
        }
        let insert = Query::insert()
            .into_table(Next::Table)
            .columns(columns)
            .values(values)
            .map_err(|error| DbErr::Migration(error.to_string()))?
            .to_owned();
        manager.exec_stmt(insert).await?;
    }
    replace(manager, Users::Table).await?;

    let mut unique: Vec<(&str, Users)> = vec![("idx_users_email", Users::Email)];
    if current {
        unique.push(("idx_users_username", Users::Username));
    }
    for (name, column) in unique {
        manager
            .create_index(
                Index::create()
                    .unique()
                    .name(name)
                    .table(Users::Table)
                    .col(column)
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

/// A username from an address that no one in `taken` has, the first free of `name`, `name-2`,
/// `name-3`… There are fewer users than suffixes.
fn unique_username(email: &str, taken: &[String]) -> String {
    let base = username_from_email(email);
    std::iter::once(base.clone())
        .chain((2_u32..=999).map(|suffix| format!("{base}-{suffix}")))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or(base)
}

const PLAY_QUEUE_COLUMNS: [&str; 11] = [
    "id",
    "source_uri",
    "shuffled",
    "version",
    "selected_position",
    "created_at",
    "user_id",
    "playlist_id",
    "client",
    "kind",
    "last_added_position",
];

fn play_queue(user_required: bool) -> TableCreateStatement {
    let mut user = ColumnDef::new(PlayQueue::UserId);
    user.big_integer();
    if user_required {
        user.not_null();
    }
    Table::create()
        .table(Next::Table)
        .col(
            ColumnDef::new(PlayQueue::Id)
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(ColumnDef::new(PlayQueue::SourceUri).text().not_null())
        .col(ColumnDef::new(PlayQueue::Shuffled).boolean().not_null())
        .col(ColumnDef::new(PlayQueue::Version).integer().not_null())
        .col(ColumnDef::new(PlayQueue::SelectedPosition).big_integer())
        .col(
            ColumnDef::new(PlayQueue::CreatedAt)
                .big_integer()
                .not_null(),
        )
        .col(&mut user)
        .col(ColumnDef::new(PlayQueue::PlaylistId).big_integer())
        .col(ColumnDef::new(PlayQueue::Client).text())
        .col(
            ColumnDef::new(PlayQueue::Kind)
                .text()
                .not_null()
                .default("video"),
        )
        .col(ColumnDef::new(PlayQueue::LastAddedPosition).big_integer())
        .to_owned()
}

async fn play_queue_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_index(
            Index::create()
                .name("idx_play_queue_created")
                .table(PlayQueue::Table)
                .col(PlayQueue::CreatedAt)
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .name("idx_play_queue_client")
                .table(PlayQueue::Table)
                .col(PlayQueue::UserId)
                .col(PlayQueue::Client)
                .col(PlayQueue::Kind)
                .to_owned(),
        )
        .await
}

const PLAYBACK_SESSION_COLUMNS: [&str; 21] = [
    "id",
    "session_key",
    "item_id",
    "part_id",
    "client",
    "product",
    "state",
    "video_decision",
    "audio_decision",
    "position_ms",
    "started_at",
    "updated_at",
    "expires_at",
    "playback_id",
    "video_bitrate",
    "audio_bitrate",
    "width",
    "height",
    "user_id",
    "terminated_at",
    "termination_reason",
];

fn playback_session(user_required: bool) -> TableCreateStatement {
    let mut user = ColumnDef::new(PlaybackSession::UserId);
    user.big_integer();
    if user_required {
        user.not_null();
    }
    Table::create()
        .table(Next::Table)
        .col(
            ColumnDef::new(PlaybackSession::Id)
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(
            ColumnDef::new(PlaybackSession::SessionKey)
                .string()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::ItemId)
                .big_integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::PartId)
                .big_integer()
                .not_null(),
        )
        .col(ColumnDef::new(PlaybackSession::Client).string())
        .col(ColumnDef::new(PlaybackSession::Product).string())
        .col(ColumnDef::new(PlaybackSession::State).string().not_null())
        .col(
            ColumnDef::new(PlaybackSession::VideoDecision)
                .string()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::AudioDecision)
                .string()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::PositionMs)
                .big_integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::StartedAt)
                .big_integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::UpdatedAt)
                .big_integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PlaybackSession::ExpiresAt)
                .big_integer()
                .not_null(),
        )
        .col(ColumnDef::new(PlaybackSession::PlaybackId).string())
        .col(ColumnDef::new(PlaybackSession::VideoBitrate).integer())
        .col(ColumnDef::new(PlaybackSession::AudioBitrate).integer())
        .col(ColumnDef::new(PlaybackSession::Width).integer())
        .col(ColumnDef::new(PlaybackSession::Height).integer())
        .col(&mut user)
        .col(ColumnDef::new(PlaybackSession::TerminatedAt).big_integer())
        .col(ColumnDef::new(PlaybackSession::TerminationReason).text())
        .to_owned()
}

async fn playback_session_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_index(
            Index::create()
                .unique()
                .name("idx_playback_session_key")
                .table(PlaybackSession::Table)
                .col(PlaybackSession::SessionKey)
                .to_owned(),
        )
        .await?;
    for (name, column) in [
        ("idx_playback_session_expiry", PlaybackSession::ExpiresAt),
        (
            "idx_playback_session_playback_id",
            PlaybackSession::PlaybackId,
        ),
    ] {
        manager
            .create_index(
                Index::create()
                    .name(name)
                    .table(PlaybackSession::Table)
                    .col(column)
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

/// The table being built in another's place.
#[derive(DeriveIden, Clone, Copy)]
enum Next {
    #[sea_orm(iden = "rebuilt_table")]
    Table,
}

#[derive(DeriveIden, Clone, Copy)]
enum Users {
    Table,
    Id,
    Username,
    Email,
    Role,
    Active,
    CreatedAt,
    UpdatedAt,
}

/// The tables keyed by a viewer, and the bookmarks' owner, which share the operator's 0.
#[derive(DeriveIden, Clone, Copy)]
enum Keyed {
    WatchState,
    Viewing,
    ViewingHistory,
    TrackChoice,
    ItemMarker,
    Viewer,
    Owner,
}

#[derive(DeriveIden, Clone, Copy)]
enum Playlist {
    Table,
    Id,
    Owner,
}

#[derive(DeriveIden, Clone, Copy)]
enum PlaylistEntry {
    Table,
    PlaylistId,
}

/// What a play queue holds, by its queue.
#[derive(DeriveIden, Clone, Copy)]
enum QueueContent {
    PlayQueueEntry,
    PlayQueueSource,
    QueueId,
}

/// The two tables whose owner was null for the operator.
#[derive(DeriveIden, Clone, Copy)]
enum Owned {
    PlayQueue,
    PlaybackSession,
    UserId,
}

#[derive(DeriveIden, Clone, Copy)]
enum PlayQueue {
    Table,
    Id,
    SourceUri,
    Shuffled,
    Version,
    SelectedPosition,
    CreatedAt,
    UserId,
    PlaylistId,
    Client,
    Kind,
    LastAddedPosition,
}

#[derive(DeriveIden, Clone, Copy)]
enum PlaybackSession {
    Table,
    Id,
    SessionKey,
    ItemId,
    PartId,
    Client,
    Product,
    State,
    VideoDecision,
    AudioDecision,
    PositionMs,
    StartedAt,
    UpdatedAt,
    ExpiresAt,
    PlaybackId,
    VideoBitrate,
    AudioBitrate,
    Width,
    Height,
    UserId,
    TerminatedAt,
    TerminationReason,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    use super::unique_username;
    use crate::migration::Migrator;

    async fn column<T: sea_orm::TryGetable>(connection: &DatabaseConnection, sql: &str) -> Vec<T> {
        connection
            .query_all(Statement::from_string(
                connection.get_database_backend(),
                sql,
            ))
            .await
            .unwrap()
            .iter()
            .map(|row| row.try_get_by_index(0).unwrap())
            .collect()
    }

    async fn insert(connection: &DatabaseConnection, sql: &str) {
        connection.execute_unprepared(sql).await.unwrap();
    }

    #[test]
    fn a_taken_username_gets_the_first_free_suffix() {
        let taken = ["admin".to_owned(), "alice".to_owned(), "alice-2".to_owned()];
        assert_eq!(unique_username("admin@example.org", &taken), "admin-2");
        assert_eq!(unique_username("alice@example.org", &taken), "alice-3");
        assert_eq!(unique_username("bob@example.org", &taken), "bob");
    }

    /// A database just before this migration: three users, and rows of the operator's and of
    /// user 5 in every table that kept them.
    async fn before_it() -> DatabaseConnection {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0030_every_caller_a_user")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        insert(
            &connection,
            "INSERT INTO users (id, email, created_at, updated_at, active) VALUES \
             (3, 'admin@example.org', 1, 2, true), (5, 'alice@example.org', 1, 2, false), \
             (6, 'alice@other.example', 1, 2, true); \
             INSERT INTO watch_state (viewer, guid, view_count) VALUES (0, 'g', 1), (5, 'g', 2); \
             INSERT INTO viewing (viewer, playback, item_id, position_ms, completed, expires_at) \
             VALUES (0, 'p', 1, 0, false, 0), (5, 'p', 1, 0, false, 0); \
             INSERT INTO playlist (id, owner, guid, title, sort_key, summary, kind, title_locked, \
             summary_locked, view_count, last_viewed_at, added_at, updated_at) VALUES \
             (1, 0, 'a', 'Mine', 'mine', '', 'video', false, false, 0, 0, 0, 0), \
             (2, 5, 'b', 'Hers', 'hers', '', 'video', false, false, 0, 0, 0, 0); \
             INSERT INTO item_marker (item_id, owner, kind, start_ms, is_final) VALUES \
             (1, NULL, 'intro', 0, true), (1, 0, 'bookmark', 5, true), (1, 5, 'bookmark', 6, true); \
             INSERT INTO play_queue (id, source_uri, shuffled, version, created_at, user_id, kind) \
             VALUES (7, 'x', false, 1, 0, NULL, 'video'), (8, 'y', false, 1, 0, 5, 'video'); \
             INSERT INTO play_queue_entry (queue_id, position, item_id) VALUES (7, 0, 1), (8, 0, 1); \
             INSERT INTO playback_session (session_key, item_id, part_id, state, video_decision, \
             audio_decision, position_ms, started_at, updated_at, expires_at, user_id) VALUES \
             ('k', 1, 1, 'playing', 'copy', 'copy', 0, 0, 0, 9, NULL), \
             ('u5~k', 1, 1, 'playing', 'copy', 'copy', 0, 0, 0, 9, 5)",
        )
        .await;
        connection
    }

    #[tokio::test]
    async fn names_the_users_and_deletes_what_was_the_operators() {
        let connection = before_it().await;

        Migrator::up(&connection, None).await.unwrap();
        assert_eq!(
            column::<String>(&connection, "SELECT username FROM users ORDER BY id").await,
            ["admin-2", "alice", "alice-2"]
        );
        assert_eq!(
            column::<String>(&connection, "SELECT role FROM users ORDER BY id").await,
            ["user", "user", "user"]
        );
        assert_eq!(
            column::<bool>(&connection, "SELECT active FROM users ORDER BY id").await,
            [true, false, true]
        );
        assert_eq!(
            column::<i64>(&connection, "SELECT viewer FROM watch_state").await,
            [5]
        );
        assert_eq!(
            column::<i64>(&connection, "SELECT viewer FROM viewing").await,
            [5]
        );
        assert_eq!(
            column::<i64>(&connection, "SELECT id FROM playlist").await,
            [2]
        );
        assert_eq!(
            column::<Option<i64>>(&connection, "SELECT owner FROM item_marker ORDER BY id").await,
            [None, Some(5)]
        );
        assert_eq!(
            column::<i64>(&connection, "SELECT id FROM play_queue").await,
            [8]
        );
        assert_eq!(
            column::<i64>(&connection, "SELECT queue_id FROM play_queue_entry").await,
            [8]
        );
        assert_eq!(
            column::<String>(&connection, "SELECT session_key FROM playback_session").await,
            ["u5~k"]
        );

        // The owners are now required, the address is not, and a username is unique.
        for refused in [
            "INSERT INTO play_queue (source_uri, shuffled, version, created_at, kind) \
             VALUES ('z', false, 1, 0, 'video')",
            "INSERT INTO users (username, email, created_at, updated_at) \
             VALUES ('alice', NULL, 0, 0)",
        ] {
            assert!(
                connection.execute_unprepared(refused).await.is_err(),
                "{refused}"
            );
        }
        insert(
            &connection,
            "INSERT INTO users (username, email, created_at, updated_at) \
             VALUES ('admin', NULL, 0, 0)",
        )
        .await;
        assert_eq!(
            column::<i64>(&connection, "SELECT id FROM users WHERE username = 'admin'").await,
            [7],
            "ids carry on from the copied rows"
        );

        // Back to before this migration, whatever came after it.
        let after = Migrator::migrations()
            .iter()
            .rev()
            .position(|migration| migration.name() == "m0030_every_caller_a_user")
            .expect("registered");
        Migrator::down(&connection, Some(u32::try_from(after + 1).unwrap()))
            .await
            .unwrap();
        assert_eq!(
            column::<String>(&connection, "SELECT email FROM users ORDER BY id").await,
            [
                "admin@example.org",
                "alice@example.org",
                "alice@other.example"
            ]
        );
    }
}
