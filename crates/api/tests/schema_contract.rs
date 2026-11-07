//! Contracts the published schema must keep.
//!
//! These guard promises clients rely on and that a careless change would quietly break: that
//! everything is documented, and that Relay's object identification works.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use open_plex_core::metadata::{ItemKind, MetadataItem};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{LibrarySection, RatingKey, SectionId, SectionKind, SectionRoot};
use serde_json::Value;

fn schema() -> ApiSchema {
    let library = InMemoryLibrary::new(vec![LibrarySection {
        id: SectionId::new(4),
        kind: SectionKind::Movie,
        title: "VF - Films".to_owned(),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: "2cd48489-54c0-41e9-ae62-f3dcb0f3c249".to_owned(),
        created_at: 1_477_835_115,
        updated_at: 1_792_916_300,
        scanned_at: Some(1_792_916_245),
        content_changed_at: 8_457_612_157_739_110_435,
        hidden: false,
    }])
    .with_roots(vec![SectionRoot {
        id: 17,
        section_id: SectionId::new(4),
        path: "/mnt/acd/Plex/Movies/Multi".to_owned(),
        created_at: 1_477_835_115,
    }]);

    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(library),
        metadata: Arc::new(InMemoryMetadata::default()),
    })
}

/// A catalog with searchable items in two libraries, for the `search` field.
fn searchable() -> ApiSchema {
    let films = section(4, SectionKind::Movie, "Films");
    let series = section(5, SectionKind::Show, "Series");

    let items = vec![
        item(
            100,
            4,
            ItemKind::Movie,
            "# Pire Soirée",
            Some("Rough Night"),
        ),
        item(101, 4, ItemKind::Movie, "Dune", None),
        item(200, 5, ItemKind::Show, "Dune: Prophecy", None),
    ];

    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![films, series])),
        metadata: Arc::new(InMemoryMetadata::new(items)),
    })
}

fn section(id: i64, kind: SectionKind, title: &str) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: title.to_owned(),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: format!("uuid-{id}"),
        created_at: 0,
        updated_at: 0,
        scanned_at: None,
        content_changed_at: 0,
        hidden: false,
    }
}

fn item(
    id: i64,
    section: i64,
    kind: ItemKind,
    title: &str,
    original_title: Option<&str>,
) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: None,
        kind,
        guid: format!("plex://item/{id}"),
        title: Some(title.to_owned()),
        title_sort: None,
        original_title: original_title.map(ToOwned::to_owned),
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: Some(1),
        year: None,
        released_on: None,
        duration_ms: None,
        rating: None,
        audience_rating: None,
        added_at: 0,
        updated_at: 0,
    }
}

async fn run_on(schema: &ApiSchema, query: &str) -> Value {
    let response = schema.execute(query).await;
    assert!(
        response.errors.is_empty(),
        "query failed: {:?}",
        response.errors
    );
    serde_json::to_value(response.data).expect("serializable")
}

async fn run(query: &str) -> Value {
    let response = schema().execute(query).await;
    assert!(
        response.errors.is_empty(),
        "query failed: {:?}",
        response.errors
    );
    serde_json::to_value(response.data).expect("serializable")
}

#[tokio::test]
async fn every_field_argument_and_enum_value_is_documented() {
    // The schema is the published contract, and an undocumented field is one a client has to
    // guess at. Enforced here rather than by review, because review forgets.
    let introspection = run(r"{ __schema { types {
            name
            fields(includeDeprecated: true) { name description args { name description } }
            enumValues(includeDeprecated: true) { name description }
        } } }")
    .await;

    let mut undocumented = Vec::new();
    let types = introspection["__schema"]["types"]
        .as_array()
        .expect("types array");

    for type_entry in types {
        let type_name = type_entry["name"].as_str().unwrap_or_default();
        // Introspection meta-types are defined by the GraphQL specification, not by us.
        if type_name.starts_with("__") {
            continue;
        }

        if let Some(fields) = type_entry["fields"].as_array() {
            for field in fields {
                let field_name = field["name"].as_str().unwrap_or_default();
                if field["description"].is_null() {
                    undocumented.push(format!("{type_name}.{field_name}"));
                }
                if let Some(args) = field["args"].as_array() {
                    for arg in args {
                        if arg["description"].is_null() {
                            let arg_name = arg["name"].as_str().unwrap_or_default();
                            undocumented.push(format!("{type_name}.{field_name}(${arg_name})"));
                        }
                    }
                }
            }
        }

        if let Some(values) = type_entry["enumValues"].as_array() {
            for value in values {
                if value["description"].is_null() {
                    let value_name = value["name"].as_str().unwrap_or_default();
                    undocumented.push(format!("{type_name}.{value_name}"));
                }
            }
        }
    }

    assert!(
        undocumented.is_empty(),
        "undocumented schema members:\n{}",
        undocumented.join("\n")
    );
}

#[tokio::test]
async fn node_refetches_the_same_object_a_list_returned() {
    // Relay's core guarantee: an id taken from any response brings the object back.
    let listed = run("{ libraries(first: 1) { edges { node { id title } } } }").await;
    let node = &listed["libraries"]["edges"][0]["node"];
    let id = node["id"].as_str().expect("global id").to_owned();

    let refetched = run(&format!(
        "{{ node(id: \"{id}\") {{ id ... on Library {{ title }} }} }}"
    ))
    .await;

    assert_eq!(refetched["node"], *node);
}

#[tokio::test]
async fn global_ids_are_opaque_and_type_qualified() {
    let listed = run("{ libraries(first: 1) { edges { node { id } } } }").await;
    let id = listed["libraries"]["edges"][0]["node"]["id"]
        .as_str()
        .expect("global id");

    // Not the bare number: a client must not be able to construct one by guessing, and the
    // API publishes no numeric field at all — the compatibility layer decodes this instead.
    assert_ne!(id, "4");
    let decoded = GlobalId::decode(id).expect("decodes");
    assert_eq!(decoded.type_name, "Library");
    assert_eq!(decoded.key, 4);
}

#[tokio::test]
async fn connections_report_cursors_and_page_boundaries() {
    let page = run(
        "{ libraries(first: 1) { edges { cursor } pageInfo { hasNextPage hasPreviousPage startCursor endCursor } } }",
    )
    .await;

    let info = &page["libraries"]["pageInfo"];
    assert_eq!(info["hasPreviousPage"], Value::Bool(false));
    // Only one library is seeded, so the first page is also the last.
    assert_eq!(info["hasNextPage"], Value::Bool(false));
    assert_eq!(info["startCursor"], info["endCursor"]);
    assert_eq!(page["libraries"]["edges"][0]["cursor"], info["startCursor"]);
}

#[tokio::test]
async fn node_rejects_a_malformed_identifier_rather_than_returning_null() {
    // Null would read as "deleted"; this is a client bug and must say so.
    let response = schema()
        .execute(r#"{ node(id: "not-base64!") { id } }"#)
        .await;
    assert!(!response.errors.is_empty());
}

#[tokio::test]
async fn node_returns_null_for_something_that_does_not_exist() {
    let id = GlobalId::encode("Library", 9999);
    let response = run(&format!("{{ node(id: \"{id}\") {{ id }} }}")).await;
    assert_eq!(response["node"], Value::Null);
}

#[tokio::test]
async fn nodes_refetches_a_batch_in_the_order_given() {
    let listed = run("{ libraries(first: 1) { edges { node { id } } } }").await;
    let id = listed["libraries"]["edges"][0]["node"]["id"]
        .as_str()
        .expect("global id")
        .to_owned();
    let missing = GlobalId::encode("Library", 9999);

    let refetched = run(&format!(
        "{{ nodes(ids: [\"{missing}\", \"{id}\", \"{id}\"]) {{ id }} }}"
    ))
    .await;

    let nodes = refetched["nodes"].as_array().expect("a list");
    // Positions line up with the input, so a missing object does not shift the rest — that
    // is the whole reason a null is returned rather than the entry being dropped.
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0], Value::Null);
    assert_eq!(nodes[1]["id"], id.as_str());
    assert_eq!(nodes[2]["id"], id.as_str());
}

#[tokio::test]
async fn nodes_refuses_a_batch_past_the_cap() {
    // A list supplied by the client needs a ceiling, for the same reason a page size does.
    let ids = (0..600)
        .map(|key| format!("\"{}\"", GlobalId::encode("Library", key)))
        .collect::<Vec<_>>()
        .join(", ");
    let response = schema()
        .execute(&format!("{{ nodes(ids: [{ids}]) {{ id }} }}"))
        .await;

    let message = response
        .errors
        .first()
        .map(|error| error.message.clone())
        .unwrap_or_default();
    assert!(message.contains("exceeds the maximum"), "{message}");
}

#[tokio::test]
async fn search_matches_a_word_prefix_and_folds_accents() {
    async fn titles(schema: &ApiSchema, term: &str) -> Vec<String> {
        let query = format!("{{ search(query: \"{term}\") {{ edges {{ node {{ title }} }} }} }}");
        run_on(schema, &query).await["search"]["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .map(|edge| {
                edge["node"]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    let schema = searchable();
    // A later word counts, which is what separates this from a title prefix.
    assert_eq!(titles(&schema, "soiree").await, ["# Pire Soirée"]);
    // And accents fold in both directions.
    assert_eq!(titles(&schema, "soirée").await, ["# Pire Soirée"]);
    // A term inside a word does not match: the index is a range, not a substring scan.
    assert!(titles(&schema, "oiree").await.is_empty());
    // A blank term matches nothing rather than everything — a cleared search box must not
    // return the whole server.
    assert!(titles(&schema, "").await.is_empty());
    assert!(titles(&schema, "   ").await.is_empty());
}

#[tokio::test]
async fn search_crosses_libraries_and_kinds() {
    let data = run_on(
        &searchable(),
        "{ search(query: \"dune\") { edges { node { __typename title library { title } } } } }",
    )
    .await;
    let edges = data["search"]["edges"].as_array().expect("edges");

    // One film and one series, from two different libraries: a single search box has to
    // reach across both.
    let kinds: Vec<&str> = edges
        .iter()
        .map(|edge| edge["node"]["__typename"].as_str().unwrap_or_default())
        .collect();
    assert!(kinds.contains(&"Movie"), "{kinds:?}");
    assert!(kinds.contains(&"Show"), "{kinds:?}");

    let libraries: std::collections::BTreeSet<&str> = edges
        .iter()
        .map(|edge| {
            edge["node"]["library"]["title"]
                .as_str()
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(libraries.len(), 2, "{libraries:?}");
}

#[tokio::test]
async fn search_results_are_refetchable_by_their_global_id() {
    // Relay's guarantee has to hold for search results too, not only for listings.
    let data = run_on(
        &searchable(),
        "{ search(query: \"dune\", first: 1) { edges { cursor node { id } } } }",
    )
    .await;
    let edge = &data["search"]["edges"][0];
    let id = edge["node"]["id"].as_str().expect("global id").to_owned();
    assert!(!edge["cursor"].as_str().unwrap_or_default().is_empty());

    let refetched = run_on(&searchable(), &format!("{{ node(id: \"{id}\") {{ id }} }}")).await;
    assert_eq!(refetched["node"]["id"], id.as_str());
}
