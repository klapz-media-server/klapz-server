//! Contracts the published schema must keep.
//!
//! These guard promises clients rely on and that a careless change would quietly break: that
//! everything is documented, and that Relay's object identification works.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use open_plex_core::metadata::{ItemKind, ItemTag, MetadataItem, Person, Tag, TagKind};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    LibrarySection, PersonId, RatingKey, SectionId, SectionKind, SectionRoot, TagId,
};
use serde_json::Value;

fn as_owner(query: impl Into<String>) -> async_graphql::Request {
    async_graphql::Request::new(query.into()).data(open_plex_core::testing::owner())
}

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
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
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
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
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
        refreshed_at: None,
    }
}

async fn run_on(schema: &ApiSchema, query: &str) -> Value {
    let response = schema.execute(as_owner(query)).await;
    assert!(
        response.errors.is_empty(),
        "query failed: {:?}",
        response.errors
    );
    serde_json::to_value(response.data).expect("serializable")
}

async fn run(query: &str) -> Value {
    let response = schema().execute(as_owner(query)).await;
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
async fn date_times_follow_the_specification_they_name() {
    let data = run(r#"{ __type(name: "DateTime") { kind description specifiedByURL } }"#).await;
    let scalar = &data["__type"];
    assert_eq!(scalar["kind"], "SCALAR");
    assert_eq!(
        scalar["specifiedByURL"],
        "https://scalars.graphql.org/andimarek/date-time"
    );
    assert!(scalar["description"].is_string(), "{scalar}");
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
        .execute(as_owner(r#"{ node(id: "not-base64!") { id } }"#))
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
        .execute(as_owner(format!("{{ nodes(ids: [{ids}]) {{ id }} }}")))
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
async fn search_ranks_its_matches_and_says_how_well_each_answers() {
    let data = run_on(
        &searchable(),
        "{ search(query: \"dune\") { edges { score titleMatch node { title } } } }",
    )
    .await;
    let edges: Vec<(String, f64, String)> = data["search"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["node"]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                edge["score"].as_f64().unwrap_or_default(),
                edge["titleMatch"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    // The title equal to the term first, however it sorts.
    assert_eq!(
        edges,
        [
            ("Dune".to_owned(), 0.93, "EXACT".to_owned()),
            (
                "Dune: Prophecy".to_owned(),
                0.53,
                "LEADING_PREFIX".to_owned()
            )
        ]
    );

    let by_title = run_on(
        &searchable(),
        "{ search(query: \"dune\", order: TITLE) { edges { node { title } } } }",
    )
    .await;
    assert_eq!(by_title["search"]["edges"][0]["node"]["title"], "Dune");
}

#[tokio::test]
async fn search_through_credits_is_empty_without_a_person_or_a_kind_that_has_one() {
    for kind in ["MOVIE", "EPISODE", "TRACK"] {
        let data = run_on(
            &searchable(),
            &format!(
                "{{ searchCredits(query: \"dune\", kind: {kind}) {{ edges {{ score node {{ id }} }} }} }}"
            ),
        )
        .await;
        assert_eq!(
            data["searchCredits"]["edges"],
            serde_json::json!([]),
            "{kind}"
        );
    }
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

/// A series with one season of two episodes, credited as the reference credits The Simpsons:
/// the series carries its genre and its regular cast, the episodes their crew and guests.
fn series_with_episodes() -> ApiSchema {
    let series = section(5, SectionKind::Show, "Series");
    let child = |id: i64, kind: ItemKind, parent: i64, title: &str| MetadataItem {
        parent_id: Some(RatingKey::new(parent)),
        ..item(id, 5, kind, title, None)
    };
    let items = vec![
        item(22_053, 5, ItemKind::Show, "The Simpsons", None),
        child(22_077, ItemKind::Season, 22_053, "Season 14"),
        child(
            22_078,
            ItemKind::Episode,
            22_077,
            "Treehouse of Horror XIII",
        ),
        child(
            22_079,
            ItemKind::Episode,
            22_077,
            "How I Spent My Strummer Vacation",
        ),
    ];

    let person = |id: i64, name: &str| Person {
        id: PersonId::new(id),
        name: name.to_owned(),
        provider_key: None,
        thumb_url: None,
    };
    let tag = |id: i64, kind: TagKind, name: &str, who: Option<Person>| Tag {
        id: TagId::new(id),
        kind,
        name: name.to_owned(),
        person: who,
    };
    let on = |item: i64, tag: Tag, role: Option<&str>| ItemTag {
        item_id: RatingKey::new(item),
        tag,
        role: role.map(ToOwned::to_owned),
    };
    let comedy = tag(4025, TagKind::Genre, "Comedy", None);
    let homer = tag(
        486_047,
        TagKind::Actor,
        "Dan Castellaneta",
        Some(person(1, "Dan Castellaneta")),
    );
    let silverman = tag(
        705_517,
        TagKind::Director,
        "David Silverman",
        Some(person(2, "David Silverman")),
    );
    let guest = tag(
        900_001,
        TagKind::Actor,
        "Mick Jagger",
        Some(person(3, "Mick Jagger")),
    );
    let tags = vec![
        on(22_053, comedy, None),
        on(22_053, homer.clone(), Some("Homer Simpson")),
        on(22_078, silverman.clone(), None),
        on(22_078, homer.clone(), Some("Homer Simpson")),
        on(22_079, silverman, None),
        on(22_079, homer, Some("Homer Simpson")),
        on(22_079, guest, Some("Mick Jagger")),
    ];

    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![series])),
        metadata: Arc::new(InMemoryMetadata::new(items).with_tags(tags)),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
    })
}

#[tokio::test]
async fn search_tags_finds_people_once_per_library_most_credited_first() {
    let schema = series_with_episodes();
    let data = run_on(
        &schema,
        r#"{ searchTags(query: "da") { edges { cursor node {
            itemCount tag { name kind person { name } } library { title }
        } } } }"#,
    )
    .await;
    let found: Vec<(String, String, u64, String)> = data["searchTags"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            let node = &edge["node"];
            (
                node["tag"]["name"].as_str().unwrap().to_owned(),
                node["tag"]["kind"].as_str().unwrap().to_owned(),
                node["itemCount"].as_u64().unwrap(),
                node["library"]["title"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    // The series and its two episodes credit Dan Castellaneta. David Silverman directed
    // the episodes only, so the library's series list would not lead to him.
    assert_eq!(
        found,
        [(
            "Dan Castellaneta".into(),
            "ACTOR".into(),
            3,
            "Series".into()
        )]
    );

    let directors = run_on(
        &schema,
        r#"{ searchTags(query: "silver", kinds: [ACTOR]) { edges { node { itemCount } } } }"#,
    )
    .await;
    assert_eq!(directors["searchTags"]["edges"], serde_json::json!([]));
}

fn names(connection: &Value) -> Vec<(String, Value)> {
    connection["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["node"]["name"].as_str().expect("a name").to_owned(),
                edge["episodeCount"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_series_gathers_the_credits_of_its_episodes_after_its_own() {
    let schema = series_with_episodes();
    let id = GlobalId::encode("Item", 22_053);
    let data = run_on(
        &schema,
        &format!(
            r#"{{ node(id: "{id}") {{ ... on Show {{
                tags {{
                  cast {{ edges {{ episodeCount character node {{ name }} }} }}
                  directors {{ edges {{ episodeCount node {{ name }} }} }}
                }}
                own: tags(includeEpisodes: false) {{ cast {{ edges {{ episodeCount node {{ name }} }} }} }}
            }} }} }}"#
        ),
    )
    .await;
    let show = &data["node"];

    // The regular cast first, counted across the episodes; then the guest of one episode.
    assert_eq!(
        names(&show["tags"]["cast"]),
        [
            ("Dan Castellaneta".to_owned(), Value::from(2)),
            ("Mick Jagger".to_owned(), Value::from(1)),
        ]
    );
    // A guest's character varies by episode, so none is claimed for the series.
    assert_eq!(
        show["tags"]["cast"]["edges"][0]["character"],
        "Homer Simpson"
    );
    assert_eq!(show["tags"]["cast"]["edges"][1]["character"], Value::Null);
    assert_eq!(
        names(&show["tags"]["directors"]),
        [("David Silverman".to_owned(), Value::from(2))]
    );
    // Asked not to, the series lists its own credits only, as an item without episodes.
    assert_eq!(
        names(&show["own"]["cast"]),
        [("Dan Castellaneta".to_owned(), Value::Null)]
    );
}

#[tokio::test]
async fn a_season_and_an_episode_inherit_their_series_genres() {
    let schema = series_with_episodes();
    for key in [22_077, 22_078] {
        let id = GlobalId::encode("Item", key);
        let data = run_on(
            &schema,
            &format!(
                r#"{{ node(id: "{id}") {{
                    ... on Season {{ tags {{ genres {{ edges {{ node {{ name }} }} }} }} }}
                    ... on Episode {{ tags {{ genres {{ edges {{ node {{ name }} }} }} }} }}
                }} }}"#
            ),
        )
        .await;
        assert_eq!(
            data["node"]["tags"]["genres"]["edges"][0]["node"]["name"], "Comedy",
            "{key}"
        );
    }
}

#[tokio::test]
async fn a_season_lists_its_episodes_crew() {
    let schema = series_with_episodes();
    let id = GlobalId::encode("Item", 22_077);
    let data = run_on(
        &schema,
        &format!(
            r#"{{ node(id: "{id}") {{ ... on Season {{
                tags {{ directors {{ edges {{ episodeCount node {{ name person {{ id }} }} }} }} }}
            }} }} }}"#
        ),
    )
    .await;
    let director = &data["node"]["tags"]["directors"]["edges"][0];
    assert_eq!(director["node"]["name"], "David Silverman");
    assert_eq!(director["episodeCount"], 2);

    // The person behind the credit is refetchable on its own.
    let person = director["node"]["person"]["id"].as_str().expect("an id");
    let refetched = run_on(
        &schema,
        &format!(r#"{{ node(id: "{person}") {{ ... on Person {{ name }} }} }}"#),
    )
    .await;
    assert_eq!(refetched["node"]["name"], "David Silverman");
}

#[tokio::test]
async fn a_tag_list_pages_by_cursor() {
    let schema = series_with_episodes();
    let id = GlobalId::encode("Item", 22_053);
    let data = run_on(
        &schema,
        &format!(
            r#"{{ node(id: "{id}") {{ ... on Show {{ tags {{ cast(first: 1) {{
                pageInfo {{ hasNextPage endCursor }}
                edges {{ node {{ name }} }}
            }} }} }} }} }}"#
        ),
    )
    .await;
    let cast = &data["node"]["tags"]["cast"];
    assert_eq!(cast["edges"][0]["node"]["name"], "Dan Castellaneta");
    assert_eq!(cast["pageInfo"]["hasNextPage"], true);

    let after = cast["pageInfo"]["endCursor"].as_str().expect("a cursor");
    let next = run_on(
        &schema,
        &format!(
            r#"{{ node(id: "{id}") {{ ... on Show {{ tags {{ cast(first: 1, after: "{after}") {{
                pageInfo {{ hasNextPage }}
                edges {{ node {{ name }} }}
            }} }} }} }} }}"#
        ),
    )
    .await;
    let cast = &next["node"]["tags"]["cast"];
    assert_eq!(cast["edges"][0]["node"]["name"], "Mick Jagger");
    assert_eq!(cast["pageInfo"]["hasNextPage"], false);
}

fn as_user(query: &str) -> async_graphql::Request {
    use open_plex_core::access::{LibraryAccess, LibraryRole};
    use open_plex_core::{Principal, SectionId, UserId};
    let libraries = std::iter::once((SectionId::new(4), LibraryRole::Viewer)).collect();
    async_graphql::Request::new(query).data(Principal {
        name: "someone".to_owned(),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(libraries),
        user: UserId::new(1),
        server_identifier: None,
    })
}

#[tokio::test]
async fn a_request_with_no_caller_sees_no_library() {
    let response = schema()
        .execute("{ libraries(first: 10) { edges { node { id } } } }")
        .await;
    let data = response.data.into_json().unwrap();
    assert_eq!(data["libraries"]["edges"], serde_json::json!([]));
}

#[tokio::test]
async fn managing_users_is_for_owners() {
    for operation in [
        r#"mutation { createUser(input: { username: "user-a", email: "a@example.org" }) { user { id } } }"#,
        r#"mutation { setUserRole(input: { userId: "VXNlcjox", role: OWNER }) { user { id } } }"#,
        r#"mutation { deleteUser(input: { userId: "VXNlcjox" }) { deleted } }"#,
        "{ users(first: 1) { edges { node { id } } } }",
    ] {
        let response = schema().execute(as_user(operation)).await;
        assert!(!response.errors.is_empty(), "{operation}");
    }
    let response = schema()
        .execute(as_owner(
            r#"mutation { createUser(input: { username: " Alice ", email: "A@example.org" }) {
                user { username email role active hasPassword builtIn } userErrors { code } } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["createUser"],
        serde_json::json!({
            "user": {
                "username": "alice",
                "email": "a@example.org",
                "role": "USER",
                "active": true,
                "hasPassword": false,
                "builtIn": false,
            },
            "userErrors": [],
        })
    );
}

const CREATE_USER: &str = r"mutation($input: CreateUserInput!) {
    createUser(input: $input) { user { id } userErrors { field code } }
}";

async fn create_user(schema: &ApiSchema, input: Value) -> Value {
    let response = schema
        .execute(
            as_owner(CREATE_USER).variables(async_graphql::Variables::from_json(
                serde_json::json!({ "input": input }),
            )),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["createUser"].clone()
}

#[tokio::test]
async fn a_username_is_checked_and_unique_whatever_its_case() {
    let schema = schema();
    let created = create_user(&schema, serde_json::json!({ "username": "Alice" })).await;
    assert_eq!(created["userErrors"], serde_json::json!([]));
    for (input, field, code) in [
        (
            serde_json::json!({ "username": "ALICE" }),
            "username",
            "USERNAME_TAKEN",
        ),
        (
            serde_json::json!({ "username": "al" }),
            "username",
            "INVALID_USERNAME",
        ),
        (
            serde_json::json!({ "username": "a@b.org" }),
            "username",
            "INVALID_USERNAME",
        ),
        (
            serde_json::json!({ "username": "bob", "email": "nope" }),
            "email",
            "INVALID_EMAIL",
        ),
    ] {
        let refused = create_user(&schema, input.clone()).await;
        assert_eq!(refused["user"], Value::Null, "{input}");
        assert_eq!(
            refused["userErrors"],
            serde_json::json!([{ "field": ["input", field], "code": code }]),
            "{input}"
        );
    }
}

const USER_CHANGES: &str = r"mutation($id: ID!) {
    demote: setUserRole(input: { userId: $id, role: USER }) { user { role } userErrors { code } }
    promote: setUserRole(input: { userId: $id, role: OWNER }) { user { role } userErrors { code } }
    deactivate: deactivateUser(input: { userId: $id }) { user { active } userErrors { code } }
    activate: activateUser(input: { userId: $id }) { user { active } userErrors { code } }
    delete: deleteUser(input: { userId: $id }) { deleted userErrors { code } }
}";

async fn change_user(schema: &ApiSchema, id: &str) -> Value {
    let response = schema
        .execute(
            as_owner(USER_CHANGES).variables(async_graphql::Variables::from_json(
                serde_json::json!({ "id": id }),
            )),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

#[tokio::test]
async fn an_owner_grants_and_revokes_the_owner_role() {
    let schema = schema();
    let created = create_user(&schema, serde_json::json!({ "username": "bob" })).await;
    let id = created["user"]["id"].as_str().unwrap();
    let changed = change_user(&schema, id).await;
    assert_eq!(changed["demote"]["user"]["role"], "USER");
    assert_eq!(changed["promote"]["user"]["role"], "OWNER");
    assert_eq!(changed["deactivate"]["user"]["active"], false);
    assert_eq!(changed["activate"]["user"]["active"], true);
    assert_eq!(changed["delete"]["deleted"], true);
    let gone = change_user(&schema, id).await;
    assert_eq!(gone["promote"]["userErrors"][0]["code"], "USER_NOT_FOUND");
    assert_eq!(gone["delete"]["userErrors"][0]["code"], "USER_NOT_FOUND");
}

#[tokio::test]
async fn the_admin_account_is_never_demoted_shut_out_or_deleted() {
    let schema = schema();
    let created = create_user(&schema, serde_json::json!({ "username": "admin" })).await;
    let id = created["user"]["id"].as_str().unwrap();
    let changed = change_user(&schema, id).await;
    for refused in ["demote", "deactivate", "delete"] {
        assert_eq!(
            changed[refused]["userErrors"],
            serde_json::json!([{ "code": "BUILT_IN_ACCOUNT" }]),
            "{refused}"
        );
    }
    assert_eq!(changed["promote"]["user"]["role"], "OWNER");
    assert_eq!(changed["activate"]["user"]["active"], true);
    assert_eq!(changed["delete"]["deleted"], false);
}

const PAIR: &str = r"mutation($input: PairExtensionInput!) {
    pairExtension(input: $input) { accountName connectionUri publishedAt }
}";

async fn connected_user(schema: &ApiSchema) -> String {
    let created = schema
        .execute(as_owner(
            r#"mutation { createUser(input: { username: "user-a", email: "a@example.org" }) { user { id } } }"#,
        ))
        .await;
    let id = created.data.into_json().unwrap()["createUser"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let connected = schema
        .execute(
            as_owner(
                "mutation($id: ID!) { connectExtension(userId: $id, extension: PLEX) { token } }",
            )
            .variables(async_graphql::Variables::from_json(
                serde_json::json!({ "id": id }),
            )),
        )
        .await;
    assert!(connected.errors.is_empty(), "{:?}", connected.errors);
    id
}

/// A request from the user `id` names, with no library.
fn as_user_with_id(id: &str, query: &str) -> async_graphql::Request {
    use open_plex_core::access::LibraryAccess;
    use open_plex_core::{Principal, UserId};
    let key = GlobalId::decode(id).unwrap().key;
    async_graphql::Request::new(query).data(Principal {
        name: "someone".to_owned(),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(std::collections::HashMap::new()),
        user: UserId::new(key),
        server_identifier: None,
    })
}

async fn second_connected_user(schema: &ApiSchema) -> String {
    let created = schema
        .execute(as_owner(
            r#"mutation { createUser(input: { username: "user-b", email: "b@example.org" }) { user { id } } }"#,
        ))
        .await;
    created.data.into_json().unwrap()["createUser"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn pair_input(user: &str, code: &str, uri: &str) -> async_graphql::Variables {
    async_graphql::Variables::from_json(serde_json::json!({ "input": {
        "userId": user, "extension": "PLEX", "code": code, "connectionUri": uri,
    }}))
}

#[tokio::test]
async fn pairing_stores_the_account_and_publishes_the_address() {
    let schema = schema();
    let user = connected_user(&schema).await;
    let response = schema
        .execute(as_owner(PAIR).variables(pair_input(
            &user,
            " claim-7 ",
            "https://a.example.org:443",
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let paired = &response.data.into_json().unwrap()["pairExtension"];
    assert_eq!(paired["accountName"], "account 7");
    assert_eq!(paired["connectionUri"], "https://a.example.org:443");
    assert!(open_plex_api::date_time::parse(paired["publishedAt"].as_str().unwrap()).is_some());
}

const MOVE: &str = r"mutation($input: MoveExtensionInput!) {
    moveExtension(input: $input) { connectionUri publishedAt }
}";

fn move_input(user: &str, uri: &str) -> async_graphql::Variables {
    async_graphql::Variables::from_json(serde_json::json!({ "input": {
        "userId": user, "extension": "PLEX", "connectionUri": uri,
    }}))
}

#[tokio::test]
async fn moving_a_paired_server_publishes_its_new_address_at_once() {
    let schema = schema();
    let user = connected_user(&schema).await;
    let paired = schema
        .execute(as_owner(PAIR).variables(pair_input(
            &user,
            "claim-7",
            "https://a.example.org:443",
        )))
        .await;
    assert!(paired.errors.is_empty(), "{:?}", paired.errors);

    let moved = schema
        .execute(as_owner(MOVE).variables(move_input(&user, "https://New.trycloudflare.com")))
        .await;
    assert!(moved.errors.is_empty(), "{:?}", moved.errors);
    let link = &moved.data.into_json().unwrap()["moveExtension"];
    // Stored in lower case, as `Host` headers are matched.
    assert_eq!(link["connectionUri"], "https://new.trycloudflare.com");
    let published = link["publishedAt"].as_str().unwrap();
    assert!(
        open_plex_api::date_time::parse(published).is_some(),
        "{link}"
    );
}

#[tokio::test]
async fn moving_refuses_a_bad_address_an_unpaired_connection_and_another_user() {
    let schema = schema();
    let user = connected_user(&schema).await;
    let other = second_connected_user(&schema).await;
    // Not paired yet.
    let response = schema
        .execute(as_owner(MOVE).variables(move_input(&user, "https://a.example.org")))
        .await;
    assert!(!response.errors.is_empty());

    let paired = schema
        .execute(as_owner(PAIR).variables(pair_input(&user, "claim-7", "https://a.example.org")))
        .await;
    assert!(paired.errors.is_empty(), "{:?}", paired.errors);
    let response = schema
        .execute(as_owner(MOVE).variables(move_input(&user, "https://a.example.org/path")))
        .await;
    assert!(!response.errors.is_empty());
    let response = schema
        .execute(
            as_user_with_id(&other, MOVE).variables(move_input(&user, "https://b.example.org")),
        )
        .await;
    assert_eq!(response.errors[0].message, "not found");
}

#[tokio::test]
async fn a_user_moves_and_unpairs_its_own_server() {
    let schema = schema();
    let user = connected_user(&schema).await;
    let paired = schema
        .execute(as_user_with_id(&user, PAIR).variables(pair_input(
            &user,
            "claim-7",
            "https://a.example.org",
        )))
        .await;
    assert!(paired.errors.is_empty(), "{:?}", paired.errors);
    let moved = schema
        .execute(as_user_with_id(&user, MOVE).variables(move_input(&user, "https://b.example.org")))
        .await;
    assert!(moved.errors.is_empty(), "{:?}", moved.errors);

    let unpair = "mutation($id: ID!) { unpairExtension(userId: $id, extension: PLEX) }";
    let variables = async_graphql::Variables::from_json(serde_json::json!({ "id": user }));
    let unpaired = schema
        .execute(as_user_with_id(&user, unpair).variables(variables))
        .await;
    assert!(unpaired.errors.is_empty(), "{:?}", unpaired.errors);
    assert_eq!(unpaired.data.into_json().unwrap()["unpairExtension"], true);
}

#[tokio::test]
async fn pairing_is_for_owners_and_the_user_itself() {
    let schema = schema();
    let user = connected_user(&schema).await;
    let other = second_connected_user(&schema).await;
    let response = schema
        .execute(as_user_with_id(&other, PAIR).variables(pair_input(
            &user,
            "claim-7",
            "https://a.example.org",
        )))
        .await;
    assert_eq!(response.errors[0].message, "not found");

    let response = schema
        .execute(as_user_with_id(&user, PAIR).variables(pair_input(
            &user,
            "claim-7",
            "https://a.example.org",
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["pairExtension"]["accountName"],
        "account 7"
    );
}

const VIEWER: &str = "{ viewer { role user { id username email role } } }";

#[tokio::test]
async fn the_viewer_is_a_user_with_its_role_or_nobody() {
    let schema = schema();
    let user = connected_user(&schema).await;

    let owner = schema
        .execute(as_owner(VIEWER))
        .await
        .data
        .into_json()
        .unwrap();
    assert_eq!(owner["viewer"]["role"], "OWNER");

    let itself = schema
        .execute(as_user_with_id(&user, VIEWER))
        .await
        .data
        .into_json()
        .unwrap();
    assert_eq!(itself["viewer"]["role"], "USER");
    assert_eq!(itself["viewer"]["user"]["id"], user.as_str());
    assert_eq!(itself["viewer"]["user"]["username"], "user-a");
    assert_eq!(itself["viewer"]["user"]["email"], "a@example.org");

    let nobody = schema.execute(VIEWER).await.data.into_json().unwrap();
    assert!(nobody["viewer"]["role"].is_null());
    assert!(nobody["viewer"]["user"].is_null());
}

#[tokio::test]
async fn pairing_refuses_a_bad_address_a_bad_code_and_a_user_not_connected() {
    let schema = schema();
    let user = connected_user(&schema).await;
    for (code, uri) in [
        ("claim-7", "https://a.example.org/path"),
        ("wrong", "https://a.example.org"),
    ] {
        let response = schema
            .execute(as_owner(PAIR).variables(pair_input(&user, code, uri)))
            .await;
        assert!(!response.errors.is_empty(), "{code} {uri}");
    }

    let created = schema
        .execute(as_owner(
            r#"mutation { createUser(input: { username: "user-b", email: "b@example.org" }) { user { id } } }"#,
        ))
        .await;
    let bare = created.data.into_json().unwrap()["createUser"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = schema
        .execute(as_owner(PAIR).variables(pair_input(&bare, "claim-7", "https://a.example.org")))
        .await;
    assert_eq!(
        response.errors[0].message,
        "this user is not connected through this extension"
    );
}

#[test]
fn no_field_gives_out_an_account_token() {
    let sdl = schema().sdl();
    let start = sdl.find("type AccountLink").unwrap();
    let body = &sdl[start..sdl[start..].find("\n}").unwrap() + start];
    assert!(!body.to_lowercase().contains("token"), "{body}");
}

/// Bull with one season of two episodes, and Soul, as in the reference's test library.
fn watchable() -> ApiSchema {
    let mut items = vec![
        item(577_986, 4, ItemKind::Movie, "Soul", None),
        item(577_987, 5, ItemKind::Show, "Bull", None),
        item(577_988, 5, ItemKind::Season, "Season 1", None),
        item(577_989, 5, ItemKind::Episode, "Episode 1", None),
        item(577_990, 5, ItemKind::Episode, "Episode 2", None),
    ];
    items[2].parent_id = Some(RatingKey::new(577_987));
    items[3].parent_id = Some(RatingKey::new(577_988));
    items[4].parent_id = Some(RatingKey::new(577_988));
    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(4, SectionKind::Movie, "Films"),
            section(5, SectionKind::Show, "Series"),
        ])),
        metadata: Arc::new(InMemoryMetadata::new(items.clone())),
        playback: Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: Arc::new(open_plex_core::testing::InMemoryWatch::over(items)),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
    })
}

fn item_id(key: i64) -> String {
    GlobalId::encode("Item", key)
}

#[tokio::test]
async fn watch_state_follows_the_callers_marks_and_reports() {
    let schema = watchable();
    let episode = item_id(577_989);
    run_on(
        &schema,
        &format!(r#"mutation {{ markWatched(id: "{episode}") {{ viewCount }} }}"#),
    )
    .await;

    let series = run_on(
        &schema,
        &format!(
            r#"{{ node(id: "{}") {{ ... on Show {{ watchState {{ viewCount isWatched }} stats {{ watchedEpisodeCount episodeCount }} }} }} }}"#,
            item_id(577_987)
        ),
    )
    .await;
    // The episode counts on its series, and is one of two watched.
    assert_eq!(series["node"]["watchState"]["viewCount"], 1);
    assert_eq!(series["node"]["stats"]["watchedEpisodeCount"], 1);
    assert_eq!(series["node"]["stats"]["episodeCount"], 2);

    let soul = item_id(577_986);
    let report = |position: i64| {
        format!(
            r#"mutation {{ reportViewing(input: {{ itemId: "{soul}", playbackId: "p", positionMs: {position}, durationMs: 6031567 }}) {{ outcome state {{ viewCount offsetMs }} }} }}"#
        )
    };
    let progress = run_on(&schema, &report(120_000)).await;
    assert_eq!(progress["reportViewing"]["outcome"], "PROGRESS");
    assert_eq!(progress["reportViewing"]["state"]["offsetMs"], 120_000);
    let done = run_on(&schema, &report(5_500_000)).await;
    assert_eq!(done["reportViewing"]["outcome"], "COMPLETED");
    assert_eq!(done["reportViewing"]["state"]["viewCount"], 1);
    assert_eq!(done["reportViewing"]["state"]["offsetMs"], Value::Null);

    let forgotten = run_on(
        &schema,
        &format!(r#"mutation {{ markUnwatched(id: "{soul}") {{ viewCount isWatched }} }}"#),
    )
    .await;
    assert_eq!(forgotten["markUnwatched"]["isWatched"], false);
}

#[tokio::test]
async fn a_caller_without_a_viewer_has_no_watch_state() {
    let schema = watchable();
    // A user given no library sees nothing it could change.
    let query = format!(
        r#"mutation {{ markWatched(id: "{}") {{ viewCount }} }}"#,
        item_id(577_986)
    );
    let response = schema
        .execute(as_user_with_id(&GlobalId::encode("User", 9), &query))
        .await;
    assert!(!response.errors.is_empty());

    // And a request with no caller at all reads no state.
    let anonymous = schema
        .execute(format!(
            r#"{{ node(id: "{}") {{ ... on Movie {{ watchState {{ viewCount }} }} }} }}"#,
            item_id(577_986)
        ))
        .await;
    let data = serde_json::to_value(anonymous.data).unwrap();
    assert!(
        data["node"].is_null() || data["node"]["watchState"].is_null(),
        "{data}"
    );
}

const HISTORY: &str = r"query($filter: HistoryFilterInput) {
    viewingHistory(first: 10, filter: $filter) {
        totalCount
        edges { node { id kind viewedAt user { id } item { id } } }
    }
}";

async fn history_of(
    schema: &ApiSchema,
    request: async_graphql::Request,
    filter: Value,
) -> (u64, Vec<Value>) {
    let request = request.variables(async_graphql::Variables::from_json(
        serde_json::json!({ "filter": filter }),
    ));
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let history = &data["viewingHistory"];
    let nodes = history["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"].clone())
        .collect();
    (history["totalCount"].as_u64().unwrap(), nodes)
}

#[tokio::test]
async fn a_user_reads_its_own_history_and_an_owner_everyones() {
    let schema = watchable();
    let soul = item_id(577_986);
    run_on(
        &schema,
        &format!(
            r#"mutation {{ markWatched(id: "{}") {{ viewCount }} }}"#,
            item_id(577_988)
        ),
    )
    .await;
    let marked = schema
        .execute(as_user(&format!(
            r#"mutation {{ markWatched(id: "{soul}") {{ viewCount }} }}"#
        )))
        .await;
    assert!(marked.errors.is_empty(), "{:?}", marked.errors);

    let (total, theirs) = history_of(&schema, as_user(HISTORY), Value::Null).await;
    assert_eq!(total, 1);
    assert_eq!(theirs[0]["item"]["id"], soul);
    assert_eq!(theirs[0]["kind"], "MARKED");

    // The season marked is one entry per episode, the owner's own.
    let (total, mine) = history_of(
        &schema,
        as_owner(HISTORY),
        serde_json::json!({ "mine": true }),
    )
    .await;
    assert_eq!(total, 2);
    let (total, _) = history_of(&schema, as_owner(HISTORY), Value::Null).await;
    assert_eq!(total, 3);
    let series = serde_json::json!({ "item": item_id(577_987) });
    assert_eq!(history_of(&schema, as_owner(HISTORY), series).await.0, 2);

    // A user naming another reads nothing, and refetches nothing of the owner's.
    let other = serde_json::json!({ "user": GlobalId::encode("User", 2) });
    assert_eq!(history_of(&schema, as_user(HISTORY), other).await.0, 0);
    let refetch = format!(
        r#"{{ node(id: "{}") {{ id }} }}"#,
        mine[0]["id"].as_str().unwrap()
    );
    let response = schema.execute(as_user(&refetch)).await;
    assert_eq!(response.data.into_json().unwrap()["node"], Value::Null);
    assert_eq!(run_on(&schema, &refetch).await["node"]["id"], mine[0]["id"]);

    let nobody = async_graphql::Request::new(HISTORY);
    assert_eq!(history_of(&schema, nobody, Value::Null).await, (0, vec![]));
}

#[tokio::test]
async fn deleting_a_history_entry_is_for_its_viewer_and_owners() {
    let schema = watchable();
    let soul = item_id(577_986);
    run_on(
        &schema,
        &format!(r#"mutation {{ markWatched(id: "{soul}") {{ viewCount }} }}"#),
    )
    .await;
    let (_, entries) = history_of(&schema, as_owner(HISTORY), Value::Null).await;
    let entry = entries[0]["id"].as_str().unwrap().to_owned();
    let delete = format!(
        r#"mutation {{ deleteHistoryEntry(input: {{ clientMutationId: "c", entry: "{entry}" }}) {{
            clientMutationId deletedEntryId userErrors {{ field code message }} }} }}"#
    );

    let refused = schema.execute(as_user(&delete)).await;
    let refused = refused.data.into_json().unwrap();
    assert_eq!(refused["deleteHistoryEntry"]["deletedEntryId"], Value::Null);
    assert_eq!(
        refused["deleteHistoryEntry"]["userErrors"][0]["code"],
        "ENTRY_NOT_FOUND"
    );
    assert_eq!(
        refused["deleteHistoryEntry"]["userErrors"][0]["field"],
        serde_json::json!(["input", "entry"])
    );

    let deleted = run_on(&schema, &delete).await;
    assert_eq!(deleted["deleteHistoryEntry"]["clientMutationId"], "c");
    assert_eq!(deleted["deleteHistoryEntry"]["deletedEntryId"], entry);
    assert_eq!(
        history_of(&schema, as_owner(HISTORY), Value::Null).await.0,
        0
    );
    // What was watched stays.
    let state = run_on(
        &schema,
        &format!(r#"{{ node(id: "{soul}") {{ ... on Movie {{ watchState {{ viewCount }} }} }} }}"#),
    )
    .await;
    assert_eq!(state["node"]["watchState"]["viewCount"], 1);
}

fn track(
    id: i64,
    kind: open_plex_core::StreamKind,
    position: i32,
    codec: &str,
) -> open_plex_core::MediaStream {
    open_plex_core::MediaStream {
        id: open_plex_core::StreamId::new(id),
        file_id: open_plex_core::PartId::new(910),
        kind,
        position,
        codec: Some(codec.to_owned()),
        language: Some("fr".to_owned()),
        channels: None,
        bitrate: Some(256_000),
        is_default: true,
        is_forced: false,
        external_path: None,
        title: None,
        stream_identifier: None,
        profile: None,
        bit_depth: None,
        traits: open_plex_core::metadata::TrackTraits::default(),
        video: None,
        audio: None,
        subtitle: None,
        required_bandwidths: Vec::new(),
    }
}

/// Soul with one file of three tracks, and an engine that records what it is asked.
fn streamable(engine: Arc<open_plex_core::testing::StubStreams>) -> ApiSchema {
    use open_plex_core::StreamKind;
    let version = open_plex_core::MediaVersion {
        id: open_plex_core::MediaId::new(900),
        item_id: RatingKey::new(577_986),
        width: Some(1920),
        height: Some(1080),
        aspect_ratio: None,
        duration_ms: Some(6_031_568),
        bitrate: Some(4_273_000),
        container: Some("mkv".to_owned()),
        video_codec: Some("h264".to_owned()),
        audio_codec: Some("aac".to_owned()),
        audio_channels: Some(2),
        frame_rate: Some(23.976),
        interlaced: false,
        optimized_for_streaming: false,
    };
    let file = open_plex_core::MediaFile {
        id: open_plex_core::PartId::new(910),
        version_id: open_plex_core::MediaId::new(900),
        position: 0,
        path: "/films/Soul.mkv".to_owned(),
        size_bytes: None,
        duration_ms: Some(6_031_568),
        layout: open_plex_core::metadata::FileLayout::default(),
        stream_url: None,
        content_hash: None,
        open_subtitles_hash: None,
    };
    let tracks = vec![
        track(920, StreamKind::Video, 0, "h264"),
        track(921, StreamKind::Audio, 1, "aac"),
        track(922, StreamKind::Subtitle, 2, "srt"),
    ];
    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(
            InMemoryLibrary::new(vec![section(4, SectionKind::Movie, "Films")]).with_roots(vec![
                SectionRoot {
                    id: 1,
                    section_id: SectionId::new(4),
                    path: "/films".to_owned(),
                    created_at: 0,
                },
            ]),
        ),
        metadata: Arc::new(
            InMemoryMetadata::new(vec![item(577_986, 4, ItemKind::Movie, "Soul", None)])
                .with_media(vec![version], vec![file], tracks),
        ),
        playback: Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: engine,
        session_locks: std::sync::Arc::default(),
    })
}

const START_STREAM: &str = r"
mutation Start($input: StartStreamInput!) {
  startStream(input: $input) {
    clientMutationId
    stream { delivery protocol videoDecision url width }
    session { id key }
    userErrors { field code }
  }
}";

async fn start(schema: &ApiSchema, input: serde_json::Value) -> Value {
    let request = as_owner(START_STREAM).variables(async_graphql::Variables::from_json(
        serde_json::json!({ "input": input }),
    ));
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    serde_json::to_value(response.data).unwrap()["startStream"].clone()
}

#[tokio::test]
async fn starting_a_stream_hands_out_its_address_and_records_its_session() {
    let engine = Arc::new(open_plex_core::testing::StubStreams::default());
    let schema = streamable(Arc::clone(&engine));
    let started = start(
        &schema,
        serde_json::json!({
            "clientMutationId": "m1",
            "file": GlobalId::encode("MediaFile", 910),
            "subtitleTrack": GlobalId::encode("MediaStream", 922),
            "subtitles": "TEXT",
            "maxVideoBitrate": 20_000_000,
            "positionMs": 60_000,
            "key": "k1",
        }),
    )
    .await;
    assert_eq!(started["clientMutationId"], "m1");
    assert_eq!(started["userErrors"], serde_json::json!([]));
    assert_eq!(started["stream"]["delivery"], "DIRECT_STREAM");
    assert_eq!(started["stream"]["protocol"], "DASH");
    assert_eq!(
        started["stream"]["url"],
        "/streams/stub/manifest.mpd?token=stub"
    );
    assert_eq!(started["session"]["key"], "k1");

    let orders = engine.started.lock().unwrap();
    let order = &orders[0];
    // Stored under the caller's own name, as every session is.
    assert_eq!(order.key, "u1000~k1");
    assert_eq!(order.source.roots, vec!["/films".to_owned()]);
    assert_eq!(order.source.audio.position, 1);
    assert_eq!(order.limits.max_video_bitrate, Some(20_000_000));
    assert_eq!(order.position_ms, 60_000);
    assert!(matches!(
        &order.subtitle,
        Some(open_plex_core::stream::SubtitleOutput::Text { track }) if track.position == 2
    ));
}

#[tokio::test]
async fn a_stream_of_a_file_nobody_can_see_is_refused_as_a_user_error() {
    let engine = Arc::new(open_plex_core::testing::StubStreams::default());
    let schema = streamable(Arc::clone(&engine));
    let refused = start(
        &schema,
        serde_json::json!({ "file": GlobalId::encode("MediaFile", 911) }),
    )
    .await;
    assert_eq!(refused["stream"], Value::Null);
    assert_eq!(
        refused["userErrors"],
        serde_json::json!([{ "field": ["input", "file"], "code": "FILE_NOT_FOUND" }])
    );
    let wrong_track = start(
        &schema,
        serde_json::json!({
            "file": GlobalId::encode("MediaFile", 910),
            "audioTrack": GlobalId::encode("MediaStream", 922),
        }),
    )
    .await;
    assert_eq!(wrong_track["userErrors"][0]["code"], "TRACK_NOT_FOUND");
    assert!(engine.started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stopping_a_stream_closes_its_session_once() {
    let schema = streamable(Arc::new(open_plex_core::testing::StubStreams::default()));
    let started = start(
        &schema,
        serde_json::json!({ "file": GlobalId::encode("MediaFile", 910), "directPlay": true }),
    )
    .await;
    assert_eq!(started["stream"]["delivery"], "DIRECT_PLAY");
    assert_eq!(started["stream"]["protocol"], Value::Null);
    let stop = r"
mutation Stop($input: StopStreamInput!) {
  stopStream(input: $input) { clientMutationId stopped }
}";
    let input = serde_json::json!({
        "input": { "clientMutationId": "m2", "session": started["session"]["id"] }
    });
    for expected in [true, false] {
        let request = as_owner(stop).variables(async_graphql::Variables::from_json(input.clone()));
        let response = schema.execute(request).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = serde_json::to_value(response.data).unwrap();
        assert_eq!(data["stopStream"]["stopped"], expected);
        assert_eq!(data["stopStream"]["clientMutationId"], "m2");
    }
}

#[tokio::test]
async fn a_conversion_falls_back_only_for_its_own_player_and_never_here() {
    let schema = streamable(Arc::new(open_plex_core::testing::StubStreams::default()));
    let started = start(
        &schema,
        serde_json::json!({ "file": GlobalId::encode("MediaFile", 910), "key": "k1" }),
    )
    .await;
    let fall_back = r"
mutation FallBack($input: FallBackStreamInput!) {
  fallBackStream(input: $input) { clientMutationId session { key } userErrors { field code } }
}";
    let run = |request: async_graphql::Request, session: &Value| {
        let input =
            serde_json::json!({ "input": { "clientMutationId": "f1", "session": session } });
        let request = request.variables(async_graphql::Variables::from_json(input));
        let schema = &schema;
        async move {
            let response = schema.execute(request).await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            serde_json::to_value(response.data).unwrap()["fallBackStream"].clone()
        }
    };
    let refused = |code: &str| {
        serde_json::json!({
            "clientMutationId": "f1",
            "session": null,
            "userErrors": [{ "field": ["input", "session"], "code": code }],
        })
    };
    let session = started["session"]["id"].clone();

    assert_eq!(
        run(as_owner(fall_back), &session).await,
        refused("NO_FALLBACK")
    );
    // Another user's session does not exist for it.
    assert_eq!(
        run(as_user(fall_back), &session).await,
        refused("SESSION_NOT_FOUND")
    );
    let item = Value::String(GlobalId::encode("Item", 900));
    assert_eq!(
        run(as_owner(fall_back), &item).await,
        refused("SESSION_NOT_FOUND")
    );

    let stop = r"mutation Stop($input: StopStreamInput!) { stopStream(input: $input) { stopped } }";
    let input = serde_json::json!({ "input": { "session": session } });
    let response = schema
        .execute(as_owner(stop).variables(async_graphql::Variables::from_json(input)))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        run(as_owner(fall_back), &session).await,
        refused("SESSION_NOT_FOUND")
    );
}

#[tokio::test]
async fn an_owner_terminates_a_session_which_its_player_can_still_read() {
    let engine = Arc::new(open_plex_core::testing::StubStreams::default());
    let schema = streamable(Arc::clone(&engine));
    let started = start(
        &schema,
        serde_json::json!({
            "file": GlobalId::encode("MediaFile", 910),
            "key": "k1",
            "playbackId": "tab",
        }),
    )
    .await;
    let session = started["session"]["id"].clone();
    let terminate = r"
mutation Terminate($input: TerminatePlaybackSessionInput!) {
  terminatePlaybackSession(input: $input) {
    clientMutationId
    session { key terminationReason }
    userErrors { field code }
  }
}";
    let input = serde_json::json!({
        "input": { "clientMutationId": "t1", "session": session, "reason": "Bedtime" }
    });
    let run = |request: async_graphql::Request| {
        let schema = &schema;
        async move { schema.execute(request).await }
    };

    // A user may stop its own stream, never terminate one.
    let refused =
        run(as_user(terminate).variables(async_graphql::Variables::from_json(input.clone()))).await;
    assert!(!refused.errors.is_empty());
    assert!(engine.stopped.lock().unwrap().is_empty());

    let response =
        run(as_owner(terminate).variables(async_graphql::Variables::from_json(input.clone())))
            .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = serde_json::to_value(response.data).unwrap();
    let payload = &data["terminatePlaybackSession"];
    assert_eq!(payload["clientMutationId"], "t1");
    assert_eq!(payload["userErrors"], serde_json::json!([]));
    assert_eq!(payload["session"]["terminationReason"], "Bedtime");
    assert_eq!(*engine.stopped.lock().unwrap(), ["u1000~k1"]);

    // No longer playing, but still there for its player to read.
    let read = format!(
        r#"{{
  playbackSessions(first: 10) {{ edges {{ node {{ key }} }} }}
  terminated: playbackSessions(first: 10, status: TERMINATED) {{
    edges {{ node {{ playbackId terminationReason terminatedAt }} }}
  }}
  node(id: "{}") {{ ... on PlaybackSession {{ terminationReason }} }}
}}"#,
        session.as_str().unwrap()
    );
    let response = run(as_owner(read)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = serde_json::to_value(response.data).unwrap();
    assert_eq!(data["playbackSessions"]["edges"], serde_json::json!([]));
    let terminated = &data["terminated"]["edges"][0]["node"];
    assert_eq!(terminated["playbackId"], "tab");
    assert_eq!(terminated["terminationReason"], "Bedtime");
    assert!(terminated["terminatedAt"].is_string());
    assert_eq!(data["node"]["terminationReason"], "Bedtime");

    let touch = r#"mutation { touchPlaybackSession(key: "k1") }"#;
    let data = serde_json::to_value(run(as_owner(touch)).await.data).unwrap();
    assert_eq!(data["touchPlaybackSession"], false);

    let again =
        run(as_owner(terminate).variables(async_graphql::Variables::from_json(input))).await;
    let data = serde_json::to_value(again.data).unwrap();
    assert_eq!(
        data["terminatePlaybackSession"]["userErrors"],
        serde_json::json!([{ "field": ["input", "session"], "code": "SESSION_NOT_FOUND" }])
    );

    // The player stops: the session goes, its stream asked to stop again, harmlessly.
    let stopped = r#"mutation { reportPlayback(input: { playbackId: "tab", state: STOPPED }) }"#;
    let data = serde_json::to_value(run(as_owner(stopped)).await.data).unwrap();
    assert_eq!(data["reportPlayback"], true);
    let data = serde_json::to_value(
        run(as_owner(
            r"{ playbackSessions(status: TERMINATED) { edges { node { key } } } }",
        ))
        .await
        .data,
    )
    .unwrap();
    assert_eq!(data["playbackSessions"]["edges"], serde_json::json!([]));
    assert_eq!(*engine.stopped.lock().unwrap(), ["u1000~k1", "u1000~k1"]);
}

#[tokio::test]
async fn a_player_stopping_stops_every_stream_of_its_playback() {
    let engine = Arc::new(open_plex_core::testing::StubStreams::default());
    let schema = streamable(Arc::clone(&engine));
    for key in ["k1", "k2"] {
        start(
            &schema,
            serde_json::json!({
                "file": GlobalId::encode("MediaFile", 910),
                "key": key,
                "playbackId": "tab",
            }),
        )
        .await;
    }
    let stopped = r#"mutation { reportPlayback(input: { playbackId: "tab", state: STOPPED }) }"#;
    let data = serde_json::to_value(schema.execute(as_owner(stopped)).await.data).unwrap();
    assert_eq!(data["reportPlayback"], true);
    let mut keys = engine.stopped.lock().unwrap().clone();
    keys.sort();
    assert_eq!(keys, ["u1000~k1", "u1000~k2"]);
    let data = serde_json::to_value(schema.execute(as_owner(stopped)).await.data).unwrap();
    assert_eq!(data["reportPlayback"], false);
}

/// The reference's two test libraries as its database held them: a film named after its file,
/// and a series whose season and episode have no title. The film's sort title is locked.
fn editable() -> ApiSchema {
    let file_name = "Soul.2020.MULTi.TRUEFRENCH.1080p.WEB.H264-EXTREME_wWw.Extreme-Down.Video";
    let film = MetadataItem {
        title_sort: Some(file_name.to_owned()),
        ..item(577_986, 27, ItemKind::Movie, file_name, None)
    };
    let child = |id: i64, kind: ItemKind, parent: i64| MetadataItem {
        parent_id: Some(RatingKey::new(parent)),
        title: None,
        year: (kind == ItemKind::Episode).then_some(2021),
        ..item(id, 28, kind, "", None)
    };
    let items = vec![
        film,
        item(577_987, 28, ItemKind::Show, "Bull", None),
        child(577_988, ItemKind::Season, 577_987),
        child(578_173, ItemKind::Episode, 577_988),
    ];
    let metadata = InMemoryMetadata::new(items).with_locked_fields(vec![(
        RatingKey::new(577_986),
        open_plex_core::edit::ItemField::TitleSort,
    )]);

    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie, "open-plex tests"),
            section(28, SectionKind::Show, "open-plex tests series"),
        ])),
        metadata: Arc::new(metadata),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
    })
}

const UPDATE_MOVIE: &str = r"
mutation Edit($input: UpdateMovieInput!) {
  updateMovie(input: $input) {
    clientMutationId
    movie { title titleSort summary year releasedOn rating lockedFields }
    userErrors { field message code }
  }
}";

async fn update_movie(
    schema: &ApiSchema,
    request: async_graphql::Request,
    input: serde_json::Value,
) -> Value {
    let variables = async_graphql::Variables::from_json(serde_json::json!({ "input": input }));
    let response = schema.execute(request.variables(variables)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    serde_json::to_value(response.data).unwrap()["updateMovie"].clone()
}

fn as_role(role: open_plex_core::access::LibraryRole) -> async_graphql::Request {
    use open_plex_core::access::LibraryAccess;
    use open_plex_core::{Principal, UserId};
    let libraries = [27, 28]
        .into_iter()
        .map(|section| (SectionId::new(section), role))
        .collect();
    async_graphql::Request::new(UPDATE_MOVIE).data(Principal {
        name: "someone".to_owned(),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(libraries),
        user: UserId::new(1),
        server_identifier: None,
    })
}

#[tokio::test]
async fn editing_a_film_writes_what_it_derives_and_changes_only_the_locks_asked() {
    let schema = editable();
    let film = GlobalId::encode("Item", 577_986);
    let edited = update_movie(
        &schema,
        as_owner(UPDATE_MOVIE),
        serde_json::json!({
            "clientMutationId": "m1",
            "id": film,
            "title": "The Soul",
            "summary": "Probe summary",
            // The UTC day of this time, which is the next day's early hours in Paris.
            "releasedOn": "2019-05-02T01:30:00+02:00",
            "rating": 8.1,
            "unlock": ["TITLE_SORT"],
            "lock": ["SUMMARY"],
        }),
    )
    .await;
    assert_eq!(
        edited,
        serde_json::json!({
            "clientMutationId": "m1",
            "movie": {
                "title": "The Soul",
                "titleSort": "Soul",
                "summary": "Probe summary",
                "year": 2019,
                "releasedOn": "2019-05-01T00:00:00Z",
                "rating": 8.1,
                "lockedFields": ["SUMMARY"],
            },
            "userErrors": [],
        })
    );

    // Read back through `node`, in a request of its own.
    let data = run_on(
        &schema,
        &format!(
            r#"{{ node(id: "{film}") {{ ... on Movie {{ title titleSort lockedFields }} }} }}"#
        ),
    )
    .await;
    assert_eq!(
        data["node"],
        serde_json::json!({ "title": "The Soul", "titleSort": "Soul", "lockedFields": ["SUMMARY"] })
    );

    let cleared = update_movie(
        &schema,
        as_owner(UPDATE_MOVIE),
        serde_json::json!({ "id": film, "summary": null, "releasedOn": null, "rating": null }),
    )
    .await;
    assert_eq!(
        cleared["movie"],
        serde_json::json!({
            "title": "The Soul",
            "titleSort": "Soul",
            "summary": null,
            "year": null,
            "releasedOn": null,
            "rating": null,
            "lockedFields": ["SUMMARY"],
        })
    );
}

#[tokio::test]
async fn only_a_librarys_admin_edits_and_the_others_find_nothing() {
    use open_plex_core::access::LibraryRole;
    let schema = editable();
    let input = serde_json::json!({
        "id": GlobalId::encode("Item", 577_986),
        "summary": "Probe summary",
    });

    let refused = update_movie(&schema, as_role(LibraryRole::Viewer), input.clone()).await;
    assert_eq!(refused["movie"], Value::Null);
    assert_eq!(
        refused["userErrors"],
        serde_json::json!([
            { "field": ["input", "id"], "message": "no such item", "code": "ITEM_NOT_FOUND" }
        ])
    );
    let applied = update_movie(&schema, as_role(LibraryRole::Admin), input).await;
    assert_eq!(applied["movie"]["summary"], "Probe summary");
}

#[tokio::test]
async fn an_edit_refuses_another_type_a_bad_value_and_a_field_the_type_lacks() {
    let schema = editable();
    let code = |payload: &Value| {
        (
            payload["userErrors"][0]["field"].clone(),
            payload["userErrors"][0]["code"].clone(),
        )
    };

    let show = update_movie(
        &schema,
        as_owner(UPDATE_MOVIE),
        serde_json::json!({ "id": GlobalId::encode("Item", 577_987), "summary": "x" }),
    )
    .await;
    assert_eq!(
        code(&show),
        (serde_json::json!(["input", "id"]), "ITEM_NOT_FOUND".into())
    );

    let film = GlobalId::encode("Item", 577_986);
    for (input, field, expected) in [
        (
            serde_json::json!({ "id": film, "releasedOn": "0000-06-01T00:00:00Z" }),
            "releasedOn",
            "INVALID_VALUE",
        ),
        (
            serde_json::json!({ "id": film, "rating": 42 }),
            "rating",
            "INVALID_VALUE",
        ),
        (
            serde_json::json!({ "id": film, "lock": ["YEAR"], "unlock": ["YEAR"] }),
            "unlock",
            "LOCKED_AND_UNLOCKED",
        ),
    ] {
        let refused = update_movie(&schema, as_owner(UPDATE_MOVIE), input).await;
        assert_eq!(
            code(&refused),
            (serde_json::json!(["input", field]), expected.into()),
            "{field}"
        );
    }

    // Not a date-time at all: refused before the mutation runs.
    let garbage = schema
        .execute(
            as_owner(UPDATE_MOVIE).variables(async_graphql::Variables::from_json(
                serde_json::json!({
                    "input": { "id": film, "releasedOn": "2019-05-01" }
                }),
            )),
        )
        .await;
    assert_eq!(garbage.errors.len(), 1, "{:?}", garbage.errors);
    assert!(
        garbage.errors[0].message.contains("RFC 3339"),
        "{}",
        garbage.errors[0].message
    );

    let response = schema
        .execute(
            as_owner(
                r"mutation Edit($input: UpdateEpisodeInput!) {
                  updateEpisode(input: $input) { episode { year } userErrors { code } }
                }",
            )
            .variables(async_graphql::Variables::from_json(serde_json::json!({
                "input": { "id": GlobalId::encode("Item", 578_173), "lock": ["GENRES"] }
            }))),
        )
        .await;
    let data = serde_json::to_value(response.data).unwrap();
    assert_eq!(
        data["updateEpisode"],
        serde_json::json!({ "episode": null, "userErrors": [{ "code": "NOT_APPLICABLE" }] })
    );
}

#[tokio::test]
async fn an_item_is_named_for_a_url_after_its_series_and_itself() {
    let schema = series_with_episodes();
    let ids = [22_053, 22_077, 22_078]
        .map(|key| format!("\"{}\"", GlobalId::encode("Item", key)))
        .join(",");
    let data = run_on(
        &schema,
        &format!("{{ nodes(ids: [{ids}]) {{ ... on Item {{ slug }} }} }}"),
    )
    .await;
    assert_eq!(
        data["nodes"],
        serde_json::json!([
            { "slug": "the-simpsons" },
            { "slug": "the-simpsons-s01" },
            { "slug": "the-simpsons-treehouse-of-horror-xiii" },
        ])
    );
}

fn sessions() -> Arc<open_plex_api::Sessions> {
    Arc::new(open_plex_api::Sessions::new(
        Arc::new(open_plex_core::account::SessionKey::generate()),
        true,
    ))
}

/// A request from nobody, as the transport sends sign-ins: no principal, the session keys.
fn anonymous(
    query: &str,
    sessions: &Arc<open_plex_api::Sessions>,
    variables: Value,
) -> async_graphql::Request {
    async_graphql::Request::new(query)
        .data(Arc::clone(sessions))
        .variables(async_graphql::Variables::from_json(variables))
}

const SIGN_UP: &str = r"mutation($input: SignUpInput!) {
    signUp(input: $input) { created userErrors { field code } }
}";
const SIGN_IN: &str = r"mutation($input: SignInInput!) {
    signIn(input: $input) { user { username email active } userErrors { field code } }
}";

fn sign_up_input(username: &str, email: Option<&str>, password: &str) -> Value {
    serde_json::json!({ "input": { "username": username, "email": email, "password": password } })
}

fn credentials(login: &str, password: &str) -> Value {
    serde_json::json!({ "input": { "login": login, "password": password } })
}

async fn run_on_as_owner(schema: &ApiSchema, query: &str) -> Value {
    let response = schema.execute(as_owner(query)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

async fn user_id_of(schema: &ApiSchema, username: &str) -> String {
    let data = run_on_as_owner(schema, "{ users(first: 10) { nodes { id username } } }").await;
    data["users"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["username"] == username)
        .map(|user| user["id"].as_str().unwrap().to_owned())
        .unwrap()
}

#[tokio::test]
async fn a_user_who_signs_up_waits_for_an_owner_then_signs_in_with_a_cookie() {
    let schema = schema();
    let sessions = sessions();
    let signed_up = schema
        .execute(anonymous(
            SIGN_UP,
            &sessions,
            sign_up_input("Alice", Some(" Alice@Example.org "), "correct horse"),
        ))
        .await;
    assert!(signed_up.errors.is_empty(), "{:?}", signed_up.errors);
    assert_eq!(
        signed_up.data.into_json().unwrap()["signUp"]["created"],
        true
    );

    let users = run_on_as_owner(
        &schema,
        "{ users(first: 10) { nodes { username email role active hasPassword } } }",
    )
    .await;
    assert_eq!(
        users["users"]["nodes"][0],
        serde_json::json!({
            "username": "alice",
            "email": "alice@example.org",
            "role": "USER",
            "active": false,
            "hasPassword": true,
        })
    );

    // The right password, on an account still waiting.
    let waiting = schema
        .execute(anonymous(
            SIGN_IN,
            &sessions,
            credentials("alice@example.org", "correct horse"),
        ))
        .await;
    assert!(waiting.http_headers.get("set-cookie").is_none());
    assert_eq!(
        waiting.data.into_json().unwrap()["signIn"]["userErrors"][0]["code"],
        "INACTIVE"
    );

    let id = user_id_of(&schema, "alice").await;
    let activate = schema
        .execute(
            as_owner(
                "mutation($id: ID!) { activateUser(input: { userId: $id }) { user { active } } }",
            )
            .variables(async_graphql::Variables::from_json(
                serde_json::json!({ "id": id }),
            )),
        )
        .await;
    assert_eq!(
        activate.data.into_json().unwrap()["activateUser"]["user"]["active"],
        true
    );

    // By its username as well as by its address, whatever their case.
    let by_username = schema
        .execute(anonymous(
            SIGN_IN,
            &sessions,
            credentials(" ALICE ", "correct horse"),
        ))
        .await;
    assert!(by_username.http_headers.get("set-cookie").is_some());
    let signed_in = schema
        .execute(anonymous(
            SIGN_IN,
            &sessions,
            credentials("ALICE@example.org", "correct horse"),
        ))
        .await;
    let cookie = signed_in
        .http_headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.starts_with("open_plex_session="), "{cookie}");
    for attribute in ["HttpOnly", "SameSite=Lax", "Secure", "Path=/"] {
        assert!(cookie.contains(attribute), "{cookie}");
    }
    assert_eq!(
        signed_in.data.into_json().unwrap()["signIn"]["user"],
        serde_json::json!({ "username": "alice", "email": "alice@example.org", "active": true })
    );

    // The cookie names this user, for the transport to read back.
    let token = cookie
        .trim_start_matches("open_plex_session=")
        .split(';')
        .next()
        .unwrap();
    let named = sessions
        .key()
        .verify(token, open_plex_core::now_epoch())
        .unwrap();
    assert_eq!(GlobalId::encode("User", named.get()), id);
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_account_get_the_same_answer() {
    let schema = schema();
    let sessions = sessions();
    schema
        .execute(anonymous(
            SIGN_UP,
            &sessions,
            sign_up_input("alice", Some("a@example.org"), "correct horse"),
        ))
        .await;
    for (email, password) in [
        ("a@example.org", "wrong horse!"),
        ("alice", "wrong horse!"),
        ("nobody@example.org", "correct horse"),
        ("nobody", "correct horse"),
        ("not an address", "x"),
    ] {
        let response = schema
            .execute(anonymous(SIGN_IN, &sessions, credentials(email, password)))
            .await;
        assert!(response.http_headers.get("set-cookie").is_none());
        let data = response.data.into_json().unwrap();
        assert!(data["signIn"]["user"].is_null());
        assert_eq!(
            data["signIn"]["userErrors"][0]["code"], "INVALID_CREDENTIALS",
            "{email}"
        );
    }
}

#[tokio::test]
async fn signing_up_refuses_a_bad_name_or_address_a_short_password_and_what_is_taken() {
    let schema = schema();
    let sessions = sessions();
    for (username, email, password, field, code) in [
        (
            "alice",
            Some("not an address"),
            "correct horse",
            "email",
            "INVALID_EMAIL",
        ),
        (
            "alice",
            Some("a@example.org"),
            "short",
            "password",
            "WEAK_PASSWORD",
        ),
        ("a b", None, "correct horse", "username", "INVALID_USERNAME"),
    ] {
        let data = schema
            .execute(anonymous(
                SIGN_UP,
                &sessions,
                sign_up_input(username, email, password),
            ))
            .await
            .data
            .into_json()
            .unwrap();
        assert_eq!(data["signUp"]["created"], false);
        assert_eq!(
            data["signUp"]["userErrors"][0],
            serde_json::json!({ "field": ["input", field], "code": code })
        );
    }
    run_on_as_owner(
        &schema,
        r#"mutation { createUser(input: { username: "user-b", email: "b@example.org" }) { user { id } } }"#,
    )
    .await;
    for (username, email, code) in [
        ("other", Some("B@example.org"), "EMAIL_TAKEN"),
        ("USER-B", None, "USERNAME_TAKEN"),
    ] {
        let taken = schema
            .execute(anonymous(
                SIGN_UP,
                &sessions,
                sign_up_input(username, email, "correct horse"),
            ))
            .await;
        assert_eq!(
            taken.data.into_json().unwrap()["signUp"]["userErrors"][0]["code"],
            code
        );
    }
}

#[tokio::test]
async fn an_identifier_is_refused_after_five_failed_sign_ins() {
    let schema = schema();
    let sessions = sessions();
    schema
        .execute(anonymous(
            SIGN_UP,
            &sessions,
            sign_up_input("alice", Some("a@example.org"), "correct horse"),
        ))
        .await;
    for _ in 0..5 {
        schema
            .execute(anonymous(
                SIGN_IN,
                &sessions,
                credentials("a@example.org", "wrong horse!"),
            ))
            .await;
    }
    // Even the right password, now.
    let response = schema
        .execute(anonymous(
            SIGN_IN,
            &sessions,
            credentials("a@example.org", "correct horse"),
        ))
        .await;
    assert_eq!(
        response.data.into_json().unwrap()["signIn"]["userErrors"][0]["code"],
        "TOO_MANY_ATTEMPTS"
    );
}

#[tokio::test]
async fn letting_users_in_is_for_owners_and_signing_out_clears_the_cookie() {
    let schema = schema();
    let sessions = sessions();
    let user = connected_user(&schema).await;
    let deactivate =
        "mutation($id: ID!) { deactivateUser(input: { userId: $id }) { user { active } } }";
    let variables = || async_graphql::Variables::from_json(serde_json::json!({ "id": user }));
    let refused = schema
        .execute(as_user_with_id(&user, deactivate).variables(variables()))
        .await;
    assert_eq!(refused.errors[0].message, "not found");
    let done = schema
        .execute(as_owner(deactivate).variables(variables()))
        .await;
    assert_eq!(
        done.data.into_json().unwrap()["deactivateUser"]["user"]["active"],
        false
    );

    let out = schema
        .execute(anonymous(
            "mutation { signOut }",
            &sessions,
            serde_json::json!({}),
        ))
        .await;
    let cookie = out
        .http_headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        cookie.starts_with("open_plex_session=;") && cookie.contains("Max-Age=0"),
        "{cookie}"
    );
}

#[tokio::test]
async fn signing_in_needs_the_session_keys_the_transport_attaches() {
    let response = schema()
        .execute(async_graphql::Request::new(SIGN_IN).variables(
            async_graphql::Variables::from_json(credentials("a@example.org", "correct horse")),
        ))
        .await;
    assert_eq!(
        response.errors[0].message,
        "signing in is not available on this server"
    );
}

#[test]
fn no_field_gives_out_a_password_or_a_session_token() {
    let sdl = schema().sdl();
    for name in ["type SignInPayload", "type SignUpPayload", "type User "] {
        let start = sdl.find(name).unwrap();
        let body = &sdl[start..sdl[start..].find("\n}").unwrap() + start];
        // Field names only: descriptions sit between triple quotes and may say anything.
        let mut in_description = false;
        let fields: Vec<String> = body
            .lines()
            .filter(|line| {
                let quotes = line.trim() == "\"\"\"";
                if quotes {
                    in_description = !in_description;
                }
                !quotes && !in_description
            })
            .filter_map(|line| line.trim().split([':', '(']).next())
            .map(str::to_lowercase)
            .collect();
        assert!(
            fields
                .iter()
                .all(|field| field != "password" && !field.contains("token")),
            "{fields:?}"
        );
    }
}

/// The GraphQL endpoint admits nobody so they can sign in: everything that reads content must
/// answer them as if there were none.
#[tokio::test]
async fn nobody_reads_nothing_from_any_entry_point() {
    let schema = series_with_episodes();
    let show = GlobalId::encode("Item", 22_053);
    let library = GlobalId::encode("Library", 5);
    let queries = [
        format!(r#"{{ node(id: "{show}") {{ id }} }}"#),
        format!(r#"{{ node(id: "{library}") {{ id }} }}"#),
        format!(r#"{{ nodes(ids: ["{show}", "{library}"]) {{ id }} }}"#),
        r#"{ search(query: "simpsons") { edges { node { id } } } }"#.to_owned(),
        r#"{ searchTags(query: "dan") { edges { node { itemCount tag { id } } } } }"#.to_owned(),
        "{ tags(kind: ACTOR) { totalCount edges { node { id } } } }".to_owned(),
        "{ libraries(first: 10) { edges { node { id } } } }".to_owned(),
        "{ recentlyAdded(kind: SHOW, first: 10) { edges { node { id } } } }".to_owned(),
        "{ continueWatching(first: 10) { edges { node { id } } } }".to_owned(),
        "{ viewer { role user { id } } }".to_owned(),
    ];
    for query in queries {
        let response = schema.execute(query.as_str()).await;
        let data = response.data.into_json().unwrap();
        let text = data.to_string();
        assert!(
            !text.contains(&show) && !text.contains(&library) && !text.contains("true"),
            "{query} → {text} {:?}",
            response.errors
        );
    }
    for denied in [
        "{ users(first: 1) { edges { node { id } } } }",
        "{ playbackSessions(first: 1) { edges { node { id } } } }",
    ] {
        let response = schema.execute(denied).await;
        assert!(!response.errors.is_empty(), "{denied}");
    }
}

/// One person credited across four libraries, one of them hidden: as an actor on films, a
/// series and a series' episode, and as a writer on one film.
fn filmography() -> ApiSchema {
    let hidden = LibrarySection {
        hidden: true,
        ..section(9, SectionKind::Movie, "VF - Films 3D")
    };
    let sections = vec![
        section(6, SectionKind::Movie, "VO - Movies"),
        section(4, SectionKind::Movie, "VF - Films"),
        section(5, SectionKind::Show, "VF - Series"),
        hidden,
    ];
    let child = |id: i64, kind: ItemKind, parent: i64| MetadataItem {
        parent_id: Some(RatingKey::new(parent)),
        ..item(id, 5, kind, "", None)
    };
    let items = vec![
        item(300, 4, ItemKind::Movie, "Zodiac", None),
        item(301, 4, ItemKind::Movie, "Alien", None),
        item(302, 6, ItemKind::Movie, "Inception", None),
        item(303, 9, ItemKind::Movie, "Alice", None),
        // Series follow their ids: Zed was added before Apple.
        item(400, 5, ItemKind::Show, "Zed", None),
        item(401, 5, ItemKind::Show, "Apple", None),
        child(402, ItemKind::Season, 401),
        child(403, ItemKind::Episode, 402),
    ];
    let person = Person {
        id: PersonId::new(70),
        name: "Leonardo DiCaprio".to_owned(),
        provider_key: Some("5d77682685719b001f3a0b79".to_owned()),
        thumb_url: None,
    };
    let credit = |id: i64, kind: TagKind| Tag {
        id: TagId::new(id),
        kind,
        name: "Leonardo DiCaprio".to_owned(),
        person: Some(person.clone()),
    };
    let on = |item: i64, tag: &Tag| ItemTag {
        item_id: RatingKey::new(item),
        tag: tag.clone(),
        role: None,
    };
    let actor = credit(531_575, TagKind::Actor);
    let writer = credit(531_576, TagKind::Writer);
    let tags = [300, 301, 302, 303, 400, 403]
        .into_iter()
        .map(|item| on(item, &actor))
        .chain([on(302, &writer)])
        .collect();

    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(sections)),
        metadata: Arc::new(InMemoryMetadata::new(items).with_tags(tags)),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        session_locks: std::sync::Arc::default(),
    })
}

fn titles(connection: &Value) -> Vec<&str> {
    connection["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| edge["node"]["title"].as_str().expect("a title"))
        .collect()
}

const WORKS: &str = r"
fragment Works on Node {
  ... on Tag { id itemCount items(first: 10) { edges { node { ... on Movie { title } ... on Show { title } } } } }
  ... on Person { id itemCount credits { edges { node { id kind } } } items(first: 10) { edges { node { ... on Movie { title } ... on Show { title } } } } }
}";

fn works_of(type_name: &str, key: i64) -> String {
    format!(
        r#"{{ node(id: "{}") {{ ...Works }} }} {WORKS}"#,
        GlobalId::encode(type_name, key)
    )
}

#[tokio::test]
async fn a_tags_works_run_library_by_title_films_by_title_series_by_arrival() {
    let data = run_on(&filmography(), &works_of("Tag", 531_575)).await;
    let tag = &data["node"];
    assert_eq!(tag["id"], GlobalId::encode("Tag", 531_575));
    // The hidden library's Alice is left out; Apple comes in through its episode.
    assert_eq!(
        titles(&tag["items"]),
        ["Alien", "Zodiac", "Zed", "Apple", "Inception"]
    );
    assert_eq!(tag["itemCount"], 5);
}

#[tokio::test]
async fn a_persons_works_join_every_role_once() {
    let data = run_on(&filmography(), &works_of("Person", 70)).await;
    let person = &data["node"];
    assert_eq!(person["id"], GlobalId::encode("Person", 70));
    let credits: Vec<&Value> = person["credits"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| &edge["node"]["kind"])
        .collect();
    assert_eq!(credits, ["ACTOR", "WRITER"]);
    // Inception, carrying both tags, once.
    assert_eq!(
        titles(&person["items"]),
        ["Alien", "Zodiac", "Zed", "Apple", "Inception"]
    );
}

#[tokio::test]
async fn a_user_sees_a_persons_works_and_credits_in_its_libraries_only() {
    let schema = filmography();
    let response = schema.execute(as_user(&works_of("Person", 70))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    // Library 4 only: the writer's credit is on a film of library 6.
    assert_eq!(titles(&data["node"]["items"]), ["Alien", "Zodiac"]);
    assert_eq!(
        data["node"]["credits"]["edges"].as_array().unwrap().len(),
        1
    );

    let writer = schema.execute(as_user(&works_of("Tag", 531_576))).await;
    assert_eq!(writer.data.into_json().unwrap()["node"], Value::Null);
}

#[tokio::test]
async fn nobody_reads_a_tag_or_a_person() {
    let schema = filmography();
    for query in [works_of("Tag", 531_575), works_of("Person", 70)] {
        let response = schema.execute(query.as_str()).await;
        assert_eq!(
            response.data.into_json().unwrap()["node"],
            Value::Null,
            "{query}"
        );
    }
}

/// The test libraries, with an episode carrying the credits and intro the reference found on
/// an episode of *The Farewell*, in the order it found them.
fn markable() -> ApiSchema {
    use open_plex_core::marker::{Marker, MarkerKind};
    let marker = |id: i64, kind, start_ms, end_ms, is_final, version| Marker {
        id: open_plex_core::MarkerId::new(id),
        item: RatingKey::new(578_173),
        kind,
        owner: None,
        start_ms,
        end_ms: Some(end_ms),
        is_final,
        version: Some(version),
        title: None,
        color: None,
    };
    let child = |id: i64, kind: ItemKind, parent: i64| MetadataItem {
        parent_id: Some(RatingKey::new(parent)),
        ..item(id, 28, kind, "Episode 1", None)
    };
    let metadata = InMemoryMetadata::new(vec![
        item(577_986, 27, ItemKind::Movie, "Soul", None),
        item(577_987, 28, ItemKind::Show, "Bull", None),
        child(577_988, ItemKind::Season, 577_987),
        child(578_173, ItemKind::Episode, 577_988),
    ])
    .with_markers(vec![
        marker(1, MarkerKind::Credits, 2_233_609, 2_397_344, true, 4),
        marker(2, MarkerKind::Intro, 136_180, 176_381, false, 5),
    ]);
    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie, "open-plex tests"),
            section(28, SectionKind::Show, "open-plex tests series"),
        ])),
        metadata: Arc::new(metadata),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        session_locks: std::sync::Arc::default(),
    })
}

/// A user given the test libraries in one role.
fn user_in(
    user: i64,
    role: open_plex_core::access::LibraryRole,
    query: &str,
) -> async_graphql::Request {
    use open_plex_core::access::LibraryAccess;
    use open_plex_core::{Principal, UserId};
    let libraries = [27, 28]
        .into_iter()
        .map(|section| (SectionId::new(section), role))
        .collect();
    async_graphql::Request::new(query).data(Principal {
        name: format!("user {user}"),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(libraries),
        user: UserId::new(user),
        server_identifier: None,
    })
}

const MARKER_FIELDS: &str = "kind startMs endMs final version title color";

async fn markers_seen(schema: &ApiSchema, request: async_graphql::Request) -> Value {
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = serde_json::to_value(response.data).unwrap();
    Value::Array(
        data["node"]["markers"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|edge| edge["node"].clone())
            .collect(),
    )
}

fn markers_query(key: i64, typename: &str) -> String {
    format!(
        r#"{{ node(id: "{}") {{ ... on {typename} {{ markers {{ edges {{ node {{ {MARKER_FIELDS} }} }} }} }} }} }}"#,
        GlobalId::encode("Item", key)
    )
}

const ADD_MARKER: &str = r"
mutation Add($input: AddMarkerInput!) {
  addMarker(input: $input) {
    clientMutationId
    added
    marker { id kind startMs endMs final version title color item { id } }
    userErrors { field message code }
  }
}";

const UPDATE_MARKER: &str = r"
mutation Update($input: UpdateMarkerInput!) {
  updateMarker(input: $input) {
    marker { kind startMs endMs final version title color }
    userErrors { field code }
  }
}";

const DELETE_MARKER: &str = r"
mutation Delete($input: DeleteMarkerInput!) {
  deleteMarker(input: $input) { clientMutationId deletedMarkerId userErrors { field code } }
}";

/// Runs a mutation and returns its one payload.
async fn mutate(schema: &ApiSchema, request: async_graphql::Request, input: Value) -> Value {
    let variables = async_graphql::Variables::from_json(serde_json::json!({ "input": input }));
    let response = schema.execute(request.variables(variables)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = serde_json::to_value(response.data).unwrap();
    data.as_object().unwrap().values().next().unwrap().clone()
}

#[tokio::test]
async fn an_episode_lists_the_markers_its_detection_found_in_order() {
    use open_plex_core::access::LibraryRole;
    let schema = markable();
    let expected = serde_json::json!([
        { "kind": "CREDITS", "startMs": 2_233_609, "endMs": 2_397_344, "final": true,
          "version": 4, "title": null, "color": null },
        { "kind": "INTRO", "startMs": 136_180, "endMs": 176_381, "final": false,
          "version": 5, "title": null, "color": null },
    ]);
    let query = markers_query(578_173, "Episode");
    assert_eq!(
        markers_seen(&schema, as_owner(query.as_str())).await,
        expected
    );
    assert_eq!(
        markers_seen(&schema, user_in(3, LibraryRole::Viewer, &query)).await,
        expected
    );
}

#[tokio::test]
async fn a_marker_is_refetched_by_node_and_hidden_from_who_cannot_read_its_item() {
    let schema = markable();
    let marker = GlobalId::encode("Marker", 2);
    let query =
        format!(r#"{{ node(id: "{marker}") {{ id ... on Marker {{ kind item {{ id }} }} }} }}"#);
    let data = run_on(&schema, &query).await;
    assert_eq!(
        data["node"],
        serde_json::json!({
            "id": marker,
            "kind": "INTRO",
            "item": { "id": GlobalId::encode("Item", 578_173) },
        })
    );
    let response = schema.execute(query.as_str()).await;
    assert_eq!(
        serde_json::to_value(response.data).unwrap()["node"],
        Value::Null
    );
}

#[tokio::test]
async fn the_works_markers_are_written_by_a_librarys_admins_only() {
    use open_plex_core::access::LibraryRole;
    let schema = markable();
    let episode = GlobalId::encode("Item", 578_173);
    let intro = serde_json::json!({
        "clientMutationId": "m1",
        "itemId": episode,
        "kind": "INTRO",
        "startMs": 1_000,
        "endMs": 30_000,
    });

    let refused = mutate(
        &schema,
        user_in(3, LibraryRole::Viewer, ADD_MARKER),
        intro.clone(),
    )
    .await;
    assert_eq!(
        refused,
        serde_json::json!({
            "clientMutationId": "m1",
            "added": false,
            "marker": null,
            "userErrors": [{ "field": ["input", "itemId"], "message": "no such item",
                             "code": "ITEM_NOT_FOUND" }],
        })
    );

    let added = mutate(&schema, user_in(3, LibraryRole::Admin, ADD_MARKER), intro).await;
    assert_eq!(added["added"], true);
    assert_eq!(
        added["marker"]["item"]["id"].as_str(),
        Some(episode.as_str())
    );
    let id = added["marker"]["id"].clone();

    // A viewer cannot move it either; an owner can, and the detection's version goes.
    let refused = mutate(
        &schema,
        user_in(3, LibraryRole::Viewer, UPDATE_MARKER),
        serde_json::json!({ "id": id, "startMs": 2_000 }),
    )
    .await;
    assert_eq!(refused["userErrors"][0]["code"], "MARKER_NOT_FOUND");
    let updated = mutate(
        &schema,
        as_owner(UPDATE_MARKER),
        serde_json::json!({ "id": GlobalId::encode("Marker", 1), "startMs": 2_200_000 }),
    )
    .await;
    assert_eq!(
        updated["marker"],
        serde_json::json!({ "kind": "CREDITS", "startMs": 2_200_000, "endMs": 2_397_344,
                            "final": true, "version": null, "title": null, "color": null })
    );

    let deleted = mutate(
        &schema,
        user_in(3, LibraryRole::Admin, DELETE_MARKER),
        serde_json::json!({ "clientMutationId": "d", "id": id }),
    )
    .await;
    assert_eq!(
        deleted,
        serde_json::json!({ "clientMutationId": "d", "deletedMarkerId": id, "userErrors": [] })
    );
    let left = markers_seen(&schema, as_owner(markers_query(578_173, "Episode"))).await;
    assert_eq!(left.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_bookmark_is_its_keepers_alone() {
    use open_plex_core::access::LibraryRole;
    let schema = markable();
    let film = markers_query(577_986, "Movie");
    let mut bookmark = serde_json::json!({
        "itemId": GlobalId::encode("Item", 577_986),
        "kind": "BOOKMARK",
        "startMs": 5_000,
        "title": "Spot",
        "color": "ff0000",
    });
    let added = mutate(
        &schema,
        user_in(3, LibraryRole::Viewer, ADD_MARKER),
        bookmark.clone(),
    )
    .await;
    assert_eq!(added["added"], true, "{added}");
    let id = added["marker"]["id"].clone();

    let mine = markers_seen(&schema, user_in(3, LibraryRole::Viewer, &film)).await;
    assert_eq!(
        mine,
        serde_json::json!([{ "kind": "BOOKMARK", "startMs": 5_000, "endMs": null,
                             "final": false, "version": null, "title": "Spot",
                             "color": "ff0000" }])
    );
    for other in [
        user_in(4, LibraryRole::Admin, &film),
        as_owner(film.as_str()),
    ] {
        assert_eq!(markers_seen(&schema, other).await, serde_json::json!([]));
    }

    // The same span again is the same bookmark, its title as it was.
    bookmark["title"] = Value::from("Other");
    let again = mutate(
        &schema,
        user_in(3, LibraryRole::Viewer, ADD_MARKER),
        bookmark,
    )
    .await;
    assert_eq!(again["added"], false);
    assert_eq!(again["marker"]["id"], id);
    assert_eq!(again["marker"]["title"], "Spot");

    let theirs = mutate(
        &schema,
        user_in(4, LibraryRole::Admin, DELETE_MARKER),
        serde_json::json!({ "id": id }),
    )
    .await;
    assert_eq!(theirs["userErrors"][0]["code"], "MARKER_NOT_FOUND");
    let renamed = mutate(
        &schema,
        user_in(3, LibraryRole::Viewer, UPDATE_MARKER),
        serde_json::json!({ "id": id, "title": null, "endMs": 9_000 }),
    )
    .await;
    assert_eq!(renamed["marker"]["title"], Value::Null);
    assert_eq!(renamed["marker"]["endMs"], 9_000);
    assert_eq!(renamed["marker"]["color"], "ff0000");
}

#[tokio::test]
async fn an_add_refuses_what_a_marker_cannot_be() {
    let schema = markable();
    let episode = GlobalId::encode("Item", 578_173);
    let cases = [
        (
            serde_json::json!({ "kind": "INTRO", "startMs": 50, "endMs": 50 }),
            "endMs",
            "EMPTY_SPAN",
        ),
        (
            serde_json::json!({ "kind": "INTRO", "startMs": -1, "endMs": 50 }),
            "startMs",
            "NEGATIVE_START",
        ),
        (
            serde_json::json!({ "kind": "CREDITS", "startMs": 50 }),
            "endMs",
            "MISSING_END",
        ),
        (
            serde_json::json!({ "kind": "INTRO", "startMs": 0, "endMs": 9, "final": true }),
            "final",
            "FINAL_NOT_CREDITS",
        ),
        (
            serde_json::json!({ "kind": "COMMERCIAL", "startMs": 0, "endMs": 9, "title": "Ad" }),
            "title",
            "NOT_BOOKMARK",
        ),
    ];
    for (mut input, field, code) in cases {
        input["itemId"] = Value::from(episode.as_str());
        let refused = mutate(&schema, as_owner(ADD_MARKER), input).await;
        assert_eq!(
            refused["userErrors"][0]["field"],
            serde_json::json!(["input", field]),
            "{code}"
        );
        assert_eq!(refused["userErrors"][0]["code"], code);
    }
    let on_a_series = mutate(
        &schema,
        as_owner(ADD_MARKER),
        serde_json::json!({
            "itemId": GlobalId::encode("Item", 577_987),
            "kind": "BOOKMARK",
            "startMs": 0,
        }),
    )
    .await;
    assert_eq!(on_a_series["userErrors"][0]["code"], "NOT_PLAYABLE");
}

async fn rate(schema: &ApiSchema, request: async_graphql::Request) -> Value {
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    serde_json::to_value(response.data).unwrap()["rateItem"].clone()
}

fn rate_item(id: &str, rating: &str, rated_at: Option<&str>) -> String {
    let rated_at = rated_at.map_or_else(String::new, |at| format!(r#", ratedAt: "{at}""#));
    format!(
        r#"mutation {{ rateItem(input: {{ clientMutationId: "m", itemId: "{id}", rating: {rating}{rated_at} }}) {{
            clientMutationId applied item {{ id }} userRating {{ value ratedAt }} userErrors {{ field code }}
        }} }}"#
    )
}

#[tokio::test]
async fn a_rating_is_the_callers_and_a_later_change_wins() {
    let schema = watchable();
    let series = item_id(577_987);
    let rated = rate(
        &schema,
        as_owner(rate_item(&series, "7.26", Some("2026-11-01T10:00:00Z"))),
    )
    .await;
    assert_eq!(rated["clientMutationId"], "m");
    assert_eq!(rated["applied"], true);
    assert_eq!(rated["item"]["id"], series.as_str());
    // The measurement, not the one decimal Plex shows.
    assert_eq!(
        rated["userRating"],
        serde_json::json!({ "value": 7.26, "ratedAt": "2026-11-01T10:00:00Z" })
    );

    // An older change leaves the rating as it is.
    let older = rate(
        &schema,
        as_owner(rate_item(&series, "4", Some("2026-10-01T10:00:00Z"))),
    )
    .await;
    assert_eq!(older["applied"], false);
    assert_eq!(older["userRating"]["value"], 7.26);

    // Nothing adds up: the season below holds no rating of its own.
    let read = |id: String| {
        format!(r#"{{ node(id: "{id}") {{ ... on Item {{ userRating {{ value }} }} }} }}"#)
    };
    let season = run_on(&schema, &read(item_id(577_988))).await;
    assert_eq!(season["node"]["userRating"], Value::Null);
    // Another user has rated nothing.
    let theirs = schema
        .execute(as_user_with_id(
            &GlobalId::encode("User", 9),
            &read(item_id(577_986)),
        ))
        .await;
    let theirs = serde_json::to_value(theirs.data).unwrap();
    assert!(
        theirs["node"].is_null() || theirs["node"]["userRating"].is_null(),
        "{theirs}"
    );

    let cleared = rate(&schema, as_owner(rate_item(&series, "null", None))).await;
    assert_eq!(cleared["applied"], true);
    assert_eq!(cleared["userRating"], Value::Null);
    let again = rate(&schema, as_owner(rate_item(&series, "null", None))).await;
    assert_eq!(again["applied"], false);
}

#[tokio::test]
async fn a_rating_off_the_scale_in_the_future_or_of_nothing_is_refused() {
    let schema = watchable();
    let soul = item_id(577_986);
    let code = |payload: &Value| payload["userErrors"][0]["code"].clone();
    let off = rate(&schema, as_owner(rate_item(&soul, "11", None))).await;
    assert_eq!(code(&off), "OFF_THE_SCALE");
    assert_eq!(
        off["userErrors"][0]["field"],
        serde_json::json!(["input", "rating"])
    );
    assert_eq!(off["applied"], false);
    let future = rate(
        &schema,
        as_owner(rate_item(&soul, "5", Some("2999-01-01T00:00:00Z"))),
    )
    .await;
    assert_eq!(code(&future), "IN_THE_FUTURE");
    // The value and the date before the item, as the reference checks them.
    let unknown = item_id(999);
    let both = rate(&schema, as_owner(rate_item(&unknown, "-1", None))).await;
    assert_eq!(code(&both), "OFF_THE_SCALE");
    let missing = rate(&schema, as_owner(rate_item(&unknown, "5", None))).await;
    assert_eq!(code(&missing), "ITEM_NOT_FOUND");
    let unread = rate(
        &schema,
        as_user_with_id(&GlobalId::encode("User", 9), &rate_item(&soul, "5", None)),
    )
    .await;
    assert_eq!(code(&unread), "ITEM_NOT_FOUND");
}

#[tokio::test]
async fn tags_of_a_kind_reach_episodes_and_only_the_callers_libraries() {
    let schema = series_with_episodes();
    let query = "{ tags(kind: DIRECTOR) { totalCount edges { node { name kind } } } }";
    let data = run_on(&schema, query).await;
    // Credited on the episodes alone, and listed all the same.
    assert_eq!(data["tags"]["totalCount"], 1);
    assert_eq!(data["tags"]["edges"][0]["node"]["name"], "David Silverman");
    assert_eq!(data["tags"]["edges"][0]["node"]["kind"], "DIRECTOR");

    // A user given the films alone sees no tag of the series.
    let response = schema.execute(as_user(query)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["tags"]["totalCount"], 0);
    assert_eq!(data["tags"]["edges"], serde_json::json!([]));
}

#[tokio::test]
async fn viewers_rank_everyone_for_an_owner_and_the_caller_alone_for_a_user() {
    let schema = watchable();
    let viewers = |id: i64, mine: bool| {
        format!(
            r#"{{ node(id: "{}") {{ ... on Movie {{ viewers(mine: {mine}) {{ edges {{ node {{ viewCount }} }} }} }} ... on Show {{ viewers(mine: {mine}) {{ edges {{ node {{ viewCount }} }} }} }} }} }}"#,
            item_id(id)
        )
    };
    let counts = |data: &Value| -> Vec<u64> {
        data["node"]["viewers"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|edge| edge["node"]["viewCount"].as_u64().unwrap())
            .collect()
    };
    // The owner marks the season, two episodes; the user marks Soul.
    run_on(
        &schema,
        &format!(
            r#"mutation {{ markWatched(id: "{}") {{ viewCount }} }}"#,
            item_id(577_988)
        ),
    )
    .await;
    let marked = schema
        .execute(as_user(&format!(
            r#"mutation {{ markWatched(id: "{}") {{ viewCount }} }}"#,
            item_id(577_986)
        )))
        .await;
    assert!(marked.errors.is_empty(), "{:?}", marked.errors);

    // The series counts its episodes; the owner sees the user's viewing of Soul.
    assert_eq!(
        counts(&run_on(&schema, &viewers(577_987, false)).await),
        [2]
    );
    assert_eq!(
        counts(&run_on(&schema, &viewers(577_986, false)).await),
        [1]
    );
    assert!(counts(&run_on(&schema, &viewers(577_986, true)).await).is_empty());

    let as_viewer = |query: String| {
        let schema = schema.clone();
        async move {
            let response = schema.execute(as_user(&query)).await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            response.data.into_json().unwrap()
        }
    };
    assert_eq!(counts(&as_viewer(viewers(577_986, false)).await), [1]);
}
