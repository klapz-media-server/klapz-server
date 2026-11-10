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

fn as_operator(query: impl Into<String>) -> async_graphql::Request {
    async_graphql::Request::new(query.into()).data(open_plex_core::Principal::operator("owner"))
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
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
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
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
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
    let response = schema.execute(as_operator(query)).await;
    assert!(
        response.errors.is_empty(),
        "query failed: {:?}",
        response.errors
    );
    serde_json::to_value(response.data).expect("serializable")
}

async fn run(query: &str) -> Value {
    let response = schema().execute(as_operator(query)).await;
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
        .execute(as_operator(r#"{ node(id: "not-base64!") { id } }"#))
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
        .execute(as_operator(format!("{{ nodes(ids: [{ids}]) {{ id }} }}")))
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
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
    })
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
    use open_plex_core::{Principal, Scope, SectionId, UserId};
    let libraries = std::iter::once((SectionId::new(4), LibraryRole::Viewer)).collect();
    async_graphql::Request::new(query).data(Principal {
        name: "someone".to_owned(),
        scope: Scope::User,
        libraries: LibraryAccess::Only(libraries),
        user: Some(UserId::new(1)),
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
async fn managing_users_is_for_the_operator() {
    for operation in [
        r#"mutation { createUser(email: "a@example.org") { id } }"#,
        "{ users(first: 1) { edges { node { id } } } }",
    ] {
        let response = schema().execute(as_user(operation)).await;
        assert!(!response.errors.is_empty(), "{operation}");
    }
    let response = schema()
        .execute(as_operator(
            r#"mutation { createUser(email: "a@example.org") { email } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

const PAIR: &str = r"mutation($input: PairExtensionInput!) {
    pairExtension(input: $input) { accountName connectionUri publishedAt }
}";

async fn connected_user(schema: &ApiSchema) -> String {
    let created = schema
        .execute(as_operator(
            r#"mutation { createUser(email: "a@example.org") { id } }"#,
        ))
        .await;
    let id = created.data.into_json().unwrap()["createUser"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let connected = schema
        .execute(
            as_operator(
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
        .execute(as_operator(PAIR).variables(pair_input(
            &user,
            " claim-7 ",
            "https://a.example.org:443",
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let paired = &response.data.into_json().unwrap()["pairExtension"];
    assert_eq!(paired["accountName"], "account 7");
    assert_eq!(paired["connectionUri"], "https://a.example.org:443");
    assert!(paired["publishedAt"].is_i64());
}

#[tokio::test]
async fn pairing_is_for_the_operator() {
    let schema = schema();
    let user = connected_user(&schema).await;
    let response = schema
        .execute(as_user(PAIR).variables(pair_input(&user, "claim-7", "https://a.example.org")))
        .await;
    assert_eq!(response.errors[0].message, "not found");
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
            .execute(as_operator(PAIR).variables(pair_input(&user, code, uri)))
            .await;
        assert!(!response.errors.is_empty(), "{code} {uri}");
    }

    let created = schema
        .execute(as_operator(
            r#"mutation { createUser(email: "b@example.org") { id } }"#,
        ))
        .await;
    let bare = created.data.into_json().unwrap()["createUser"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = schema
        .execute(as_operator(PAIR).variables(pair_input(&bare, "claim-7", "https://a.example.org")))
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
        access: Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: Arc::new(open_plex_core::testing::InMemoryWatch::over(items)),
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
    // A visitor at a published address sees no library and has no state to change.
    let visitor = open_plex_core::Principal::visitor_of("server".to_owned());
    let query = format!(
        r#"mutation {{ markWatched(id: "{}") {{ viewCount }} }}"#,
        item_id(577_986)
    );
    let response = schema
        .execute(async_graphql::Request::new(query).data(visitor))
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
