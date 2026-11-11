//! Playlists, each owned by one viewer, and their entries in order; and the playlist a play
//! queue plays, which it names as the reference names it.
//!
//! An entry names its item without a foreign key: an item that leaves the library and comes
//! back under its id, as a re-import brings it back, finds its place in the playlist again.
//! Until then the entry is hidden, because every read joins it to the item.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let big = |column: Playlist| ColumnDef::new(column).big_integer().not_null().to_owned();
        manager
            .create_table(
                Table::create()
                    .table(Playlist::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Playlist::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(big(Playlist::Owner))
                    .col(ColumnDef::new(Playlist::Guid).string_len(64).not_null())
                    .col(ColumnDef::new(Playlist::Title).text().not_null())
                    .col(ColumnDef::new(Playlist::SortKey).text().not_null())
                    .col(ColumnDef::new(Playlist::Summary).text().not_null())
                    .col(ColumnDef::new(Playlist::Kind).string_len(16).not_null())
                    .col(ColumnDef::new(Playlist::RuleSectionId).big_integer())
                    .col(ColumnDef::new(Playlist::RuleListing).text())
                    .col(ColumnDef::new(Playlist::SourceUri).text())
                    .col(ColumnDef::new(Playlist::TitleLocked).boolean().not_null())
                    .col(ColumnDef::new(Playlist::SummaryLocked).boolean().not_null())
                    .col(big(Playlist::ViewCount))
                    .col(big(Playlist::LastViewedAt))
                    .col(big(Playlist::AddedAt))
                    .col(big(Playlist::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_playlist_owner_sort_key")
                    .table(Playlist::Table)
                    .col(Playlist::Owner)
                    .col(Playlist::SortKey)
                    .to_owned(),
            )
            .await?;
        let entry =
            |column: PlaylistEntry| ColumnDef::new(column).big_integer().not_null().to_owned();
        manager
            .create_table(
                Table::create()
                    .table(PlaylistEntry::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaylistEntry::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(entry(PlaylistEntry::PlaylistId))
                    .col(entry(PlaylistEntry::ItemId))
                    .col(entry(PlaylistEntry::Position))
                    .col(entry(PlaylistEntry::AddedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PlaylistEntry::Table, PlaylistEntry::PlaylistId)
                            .to(Playlist::Table, Playlist::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // A video playlist holds an item once: the reference drops a repeat silently.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("idx_playlist_entry_item")
                    .table(PlaylistEntry::Table)
                    .col(PlaylistEntry::PlaylistId)
                    .col(PlaylistEntry::ItemId)
                    .to_owned(),
            )
            .await?;
        // Not unique: a renumbering rewrites positions one row at a time, and a backend
        // checking uniqueness per statement would refuse the states in between.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_playlist_entry_position")
                    .table(PlaylistEntry::Table)
                    .col(PlaylistEntry::PlaylistId)
                    .col(PlaylistEntry::Position)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PlayQueue::Table)
                    .add_column(ColumnDef::new(PlayQueue::PlaylistId).big_integer())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PlayQueue::Table)
                    .drop_column(PlayQueue::PlaylistId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(PlaylistEntry::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Playlist::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden, Clone, Copy)]
enum Playlist {
    Table,
    Id,
    Owner,
    Guid,
    Title,
    SortKey,
    Summary,
    Kind,
    RuleSectionId,
    RuleListing,
    SourceUri,
    TitleLocked,
    SummaryLocked,
    ViewCount,
    LastViewedAt,
    AddedAt,
    UpdatedAt,
}

#[derive(DeriveIden, Clone, Copy)]
enum PlaylistEntry {
    Table,
    Id,
    PlaylistId,
    ItemId,
    Position,
    AddedAt,
}

#[derive(DeriveIden)]
enum PlayQueue {
    Table,
    PlaylistId,
}
