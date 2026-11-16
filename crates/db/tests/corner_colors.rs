//! Storage tests for an item's backdrop colors, with the values the reference stores for
//! *Retour vers le futur III* (item 21812).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::backdrop::{CornerColors, Rgb};
use open_plex_core::metadata::MetadataRepository;
use open_plex_core::RatingKey;
use open_plex_db::entity::{item_corner_colors, library_section, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILM: i64 = 21_812;
const UNANALYSED: i64 = 21_813;

fn film(id: i64) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(4),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set("movie".to_owned()),
        guid: ActiveValue::Set(format!("plex://movie/{id}")),
        title: ActiveValue::Set(Some("Retour vers le futur III".to_owned())),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set("retour vers le futur iii".to_owned()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(None),
        year: ActiveValue::Set(Some(1990)),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    }
}

async fn repository() -> (SqlMetadataRepository, DatabaseConnection) {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(4),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("VF - Films".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set("2cd48489-54c0-41e9-ae62-f3dcb0f3c249".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .expect("section");
    write::insert_items(&connection, vec![film(FILM), film(UNANALYSED)])
        .await
        .expect("items");
    item_corner_colors::Entity::insert(item_corner_colors::ActiveModel {
        item_id: ActiveValue::Set(FILM),
        top_left: ActiveValue::Set(0x001a_2c53),
        top_right: ActiveValue::Set(0x0081_502b),
        bottom_right: ActiveValue::Set(0x0058_421c),
        bottom_left: ActiveValue::Set(0x008f_461d),
    })
    .exec(&connection)
    .await
    .expect("colors");
    (SqlMetadataRepository::new(connection.clone()), connection)
}

#[tokio::test]
async fn reads_the_colors_of_the_items_that_have_them() {
    let (repository, _) = repository().await;
    let colors = repository
        .corner_colors(&[RatingKey::new(FILM), RatingKey::new(UNANALYSED)])
        .await
        .unwrap();
    assert_eq!(
        colors,
        vec![(
            RatingKey::new(FILM),
            CornerColors {
                top_left: Rgb::new(0x1a, 0x2c, 0x53),
                top_right: Rgb::new(0x81, 0x50, 0x2b),
                bottom_right: Rgb::new(0x58, 0x42, 0x1c),
                bottom_left: Rgb::new(0x8f, 0x46, 0x1d),
            }
        )]
    );
    assert!(repository.corner_colors(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_colors_go_with_their_item() {
    let (repository, connection) = repository().await;
    metadata_item::Entity::delete_by_id(FILM)
        .exec(&connection)
        .await
        .unwrap();
    assert!(repository
        .corner_colors(&[RatingKey::new(FILM)])
        .await
        .unwrap()
        .is_empty());
}
