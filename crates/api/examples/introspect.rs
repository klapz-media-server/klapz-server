//! Prints the full introspection result as JSON.
//!
//! Schema visualizers and client tooling consume this rather than SDL. Generated from the
//! code and without a running server, so it cannot drift from what the server would answer.
//!
//! Usage: `cargo run -p open-plex-api --example introspect > docs/introspection.json`

// Build-time tooling: a failure here must stop the build loudly rather than emit a
// half-written schema file that something downstream would then trust.
#![allow(clippy::expect_used)]

use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};

/// The canonical introspection query, as GraphQL tooling issues it.
const INTROSPECTION_QUERY: &str = r"
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
    directives {
      name
      description
      locations
      args { ...InputValue }
    }
  }
}

fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}

fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType { kind name }
            }
          }
        }
      }
    }
  }
}
";

#[tokio::main]
async fn main() {
    // The schema's shape does not depend on its data, so an empty repository is enough.
    let schema = build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::default()),
        metadata: Arc::new(InMemoryMetadata::default()),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
    });

    let response = schema.execute(INTROSPECTION_QUERY).await;
    assert!(
        response.errors.is_empty(),
        "introspection failed: {:?}",
        response.errors
    );

    // Visualizers expect the whole response envelope, not just the `__schema` payload.
    let envelope = serde_json::json!({ "data": response.data });
    println!(
        "{}",
        serde_json::to_string_pretty(&envelope).expect("serializable")
    );
}
