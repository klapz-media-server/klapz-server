//! Parses a hand-written SDL file and reports syntax errors.
//!
//! Schema proposals are reviewed before they are implemented, and a proposal that does not
//! parse wastes everyone's time. This checks syntax only — it cannot verify that referenced
//! types exist, since a proposal names types defined elsewhere.
//!
//! Usage: `cargo run -p open-plex-api --example validate_sdl -- docs/proposal-items.graphql`

// Build-time tooling: a failure here must stop the build loudly.
#![allow(clippy::expect_used)]

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: validate_sdl <file.graphql>");
    let source = std::fs::read_to_string(&path).expect("readable file");

    match async_graphql::parser::parse_schema(&source) {
        Ok(document) => {
            println!("{path}: parses, {} definitions", document.definitions.len());
        }
        Err(error) => {
            eprintln!("{path}: {error}");
            std::process::exit(1);
        }
    }
}
