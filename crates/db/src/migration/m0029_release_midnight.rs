//! Release dates as integers: the day's midnight UTC in Unix seconds, where `m0002_metadata`
//! stored `YYYY-MM-DD` text. Every other time in the schema is already Unix seconds, and a
//! date written as text compares correctly only as long as nothing writes it another way.
//!
//! The rows are converted in place, through a column of the new type that then takes the old
//! one's name: `SQLite` cannot change a column's type. Each distinct date is converted once,
//! by `core`'s calendar rather than an engine's date functions, which no two engines share,
//! and written to every row carrying it through a temporary index. A text that is no date,
//! which nothing writes, becomes no date.
//!
//! The smart playlists' and collections' rules are converted too. They keep the official
//! API's own listing input as JSON, whose `releasedFrom`, `lastViewedFrom` and `lastViewedTo`
//! were a date and two numbers in strings and are now RFC 3339 date-times; left alone, a rule
//! would no longer read.

use klapz_core::edit::CalendarDate;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Value};
use serde_json::{Map, Value as Json};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        convert(
            manager,
            ColumnDef::new(MetadataItem::ReleasedOnNext)
                .big_integer()
                .to_owned(),
            |stored| {
                let Value::String(Some(text)) = stored else {
                    return None;
                };
                let midnight = CalendarDate::parse(text)?.midnight();
                Some(Value::BigInt(Some(midnight)))
            },
        )
        .await?;
        rewrite_rules(manager, |filter| {
            rewrite_time(filter, "releasedFrom", |text| {
                written_time(CalendarDate::parse(text)?.midnight())
            });
            for key in ["lastViewedFrom", "lastViewedTo"] {
                rewrite_time(filter, key, |text| written_time(text.parse().ok()?));
            }
        })
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        convert(
            manager,
            ColumnDef::new(MetadataItem::ReleasedOnNext)
                .string()
                .to_owned(),
            |stored| {
                let Value::BigInt(Some(midnight)) = stored else {
                    return None;
                };
                let date = CalendarDate::containing(*midnight)?;
                Some(Value::String(Some(Box::new(date.to_string()))))
            },
        )
        .await?;
        rewrite_rules(manager, |filter| {
            rewrite_time(filter, "releasedFrom", |text| {
                let date = CalendarDate::containing(read_time(text)?)?;
                Some(date.to_string())
            });
            for key in ["lastViewedFrom", "lastViewedTo"] {
                rewrite_time(filter, key, |text| Some(read_time(text)?.to_string()));
            }
        })
        .await
    }
}

/// Replaces `released_on` with a column of `next`'s type, each distinct value rewritten by
/// `rewrite`; `None` leaves the rows carrying it without a date.
async fn convert(
    manager: &SchemaManager<'_>,
    mut next: ColumnDef,
    rewrite: fn(&Value) -> Option<Value>,
) -> Result<(), DbErr> {
    let connection = manager.get_connection();
    let backend = manager.get_database_backend();
    manager
        .alter_table(
            Table::alter()
                .table(MetadataItem::Table)
                .add_column(&mut next)
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .name(INDEX)
                .table(MetadataItem::Table)
                .col(MetadataItem::ReleasedOn)
                .to_owned(),
        )
        .await?;

    let distinct = Query::select()
        .distinct()
        .column(MetadataItem::ReleasedOn)
        .from(MetadataItem::Table)
        .and_where(Expr::col(MetadataItem::ReleasedOn).is_not_null())
        .to_owned();
    for row in connection.query_all(backend.build(&distinct)).await? {
        let stored = row
            .try_get_by_index::<Option<String>>(0)
            .map(|text| Value::String(text.map(Box::new)))
            .or_else(|_| row.try_get_by_index::<Option<i64>>(0).map(Value::BigInt))?;
        let Some(rewritten) = rewrite(&stored) else {
            continue;
        };
        let update = Query::update()
            .table(MetadataItem::Table)
            .value(MetadataItem::ReleasedOnNext, rewritten)
            .and_where(Expr::col(MetadataItem::ReleasedOn).eq(stored))
            .to_owned();
        manager.exec_stmt(update).await?;
    }

    manager
        .drop_index(
            Index::drop()
                .name(INDEX)
                .table(MetadataItem::Table)
                .to_owned(),
        )
        .await?;
    manager
        .alter_table(
            Table::alter()
                .table(MetadataItem::Table)
                .drop_column(MetadataItem::ReleasedOn)
                .to_owned(),
        )
        .await?;
    manager
        .alter_table(
            Table::alter()
                .table(MetadataItem::Table)
                .rename_column(MetadataItem::ReleasedOnNext, MetadataItem::ReleasedOn)
                .to_owned(),
        )
        .await
}

/// Rewrites the `filter` of every stored rule with `rewrite`.
async fn rewrite_rules(
    manager: &SchemaManager<'_>,
    rewrite: fn(&mut Map<String, Json>),
) -> Result<(), DbErr> {
    let connection = manager.get_connection();
    let backend = manager.get_database_backend();
    for table in [Rules::Playlist, Rules::Collection] {
        let stored = Query::select()
            .columns([Rules::Id, Rules::RuleListing])
            .from(table)
            .and_where(Expr::col(Rules::RuleListing).is_not_null())
            .to_owned();
        for row in connection.query_all(backend.build(&stored)).await? {
            let id: i64 = row.try_get_by_index(0)?;
            let listing: String = row.try_get_by_index(1)?;
            let Ok(mut rule) = serde_json::from_str::<Json>(&listing) else {
                continue;
            };
            let Some(Json::Object(filter)) = rule.get_mut("filter") else {
                continue;
            };
            rewrite(filter);
            let rewritten = rule.to_string();
            if rewritten == listing {
                continue;
            }
            let update = Query::update()
                .table(table)
                .value(Rules::RuleListing, rewritten)
                .and_where(Expr::col(Rules::Id).eq(id))
                .to_owned();
            manager.exec_stmt(update).await?;
        }
    }
    Ok(())
}

/// Replaces the string at `key` by what `rewrite` makes of it, unless it makes nothing.
fn rewrite_time(
    filter: &mut Map<String, Json>,
    key: &str,
    rewrite: impl Fn(&str) -> Option<String>,
) {
    if let Some(Json::String(text)) = filter.get_mut(key) {
        if let Some(rewritten) = rewrite(text) {
            *text = rewritten;
        }
    }
}

const SECONDS_PER_DAY: i64 = 86_400;

/// A time as the official API writes a `DateTime`: `2024-05-01T12:34:56Z`.
fn written_time(seconds: i64) -> Option<String> {
    let date = CalendarDate::containing(seconds)?;
    let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    Some(format!(
        "{date}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    ))
}

/// The inverse of [`written_time`], which is the only form a stored rule holds.
fn read_time(text: &str) -> Option<i64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let mut parts = time.split(':').map(|part| part.parse::<i64>().ok());
    let (Some(Some(hours)), Some(Some(minutes)), Some(Some(seconds)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some(CalendarDate::parse(date)?.midnight() + hours * 3600 + minutes * 60 + seconds)
}

/// Seeks each date's rows while converting, and goes with the old column.
const INDEX: &str = "idx_metadata_item_released_on_conversion";

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    ReleasedOn,
    ReleasedOnNext,
}

/// The two tables that keep rules, by the columns they share.
#[derive(DeriveIden, Clone, Copy)]
enum Rules {
    Playlist,
    Collection,
    Id,
    RuleListing,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    use super::{read_time, written_time};
    use crate::migration::Migrator;

    async fn column<T: sea_orm::TryGetable>(
        connection: &DatabaseConnection,
        sql: &str,
    ) -> Vec<Option<T>> {
        let rows = connection
            .query_all(Statement::from_string(
                connection.get_database_backend(),
                sql,
            ))
            .await
            .unwrap();
        rows.iter()
            .map(|row| row.try_get_by_index(0).unwrap())
            .collect()
    }

    const RELEASED: &str = "SELECT released_on FROM metadata_item ORDER BY id";
    const RULES: &str = "SELECT rule_listing FROM playlist ORDER BY id";

    #[tokio::test]
    async fn converts_the_dates_stored_before_it_and_back() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0029_release_midnight")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        let old_rule = r#"{"libraryId":"TGlicmFyeTo0","filter":{"releasedFrom":"2025-11-10","lastViewedFrom":"1700000000","yearFrom":2000}}"#;
        connection
            .execute_unprepared(&format!(
                "INSERT INTO library_section (id, kind, title, language, uuid, created_at, \
                 updated_at, content_changed_at, hidden) \
                 VALUES (4, 'movie', 'Films', 'en', 'u', 0, 0, 0, false); \
                 INSERT INTO metadata_item \
                 (id, library_section_id, kind, guid, sort_key, released_on, added_at, updated_at) \
                 VALUES (1, 4, 'movie', 'a', 'avatar', '2009-12-16', 0, 0), \
                 (2, 4, 'movie', 'b', 'avatar 2', '2009-12-16', 0, 0), \
                 (3, 4, 'movie', 'c', 'metropolis', '1927-01-10', 0, 0), \
                 (4, 4, 'movie', 'd', 'undated', NULL, 0, 0), \
                 (5, 4, 'movie', 'e', 'garbled', 'soon', 0, 0); \
                 INSERT INTO playlist (id, owner, guid, title, sort_key, summary, kind, \
                 rule_section_id, rule_listing, title_locked, summary_locked, view_count, \
                 last_viewed_at, added_at, updated_at) VALUES \
                 (1, 0, 'g1', 'Recent', 'recent', '', 'video', 4, '{old_rule}', false, false, \
                 0, 0, 0, 0), \
                 (2, 0, 'g2', 'Chosen', 'chosen', '', 'video', NULL, NULL, false, false, \
                 0, 0, 0, 0)"
            ))
            .await
            .unwrap();

        Migrator::up(&connection, Some(1)).await.unwrap();
        assert_eq!(
            column::<i64>(&connection, RELEASED).await,
            [
                Some(1_260_921_600),
                Some(1_260_921_600),
                Some(-1_356_220_800),
                None,
                None,
            ]
        );
        assert_eq!(
            column::<String>(&connection, RULES).await,
            [
                Some(
                    r#"{"libraryId":"TGlicmFyeTo0","filter":{"releasedFrom":"2025-11-10T00:00:00Z","lastViewedFrom":"2023-11-14T22:13:20Z","yearFrom":2000}}"#
                        .to_owned()
                ),
                None
            ]
        );

        Migrator::down(&connection, Some(1)).await.unwrap();
        assert_eq!(
            column::<String>(&connection, RELEASED).await,
            [
                Some("2009-12-16".to_owned()),
                Some("2009-12-16".to_owned()),
                Some("1927-01-10".to_owned()),
                None,
                None,
            ]
        );
        assert_eq!(
            column::<String>(&connection, RULES).await,
            [Some(old_rule.to_owned()), None]
        );
    }

    #[test]
    fn writes_and_reads_the_api_form_of_a_time() {
        for (seconds, text) in [
            (0, "1970-01-01T00:00:00Z"),
            (1_714_566_896, "2024-05-01T12:34:56Z"),
            (-1, "1969-12-31T23:59:59Z"),
        ] {
            assert_eq!(written_time(seconds).as_deref(), Some(text));
            assert_eq!(read_time(text), Some(seconds));
        }
        assert_eq!(read_time("2024-05-01T12:34:56+02:00"), None);
        assert_eq!(read_time("2024-05-01"), None);
    }
}
