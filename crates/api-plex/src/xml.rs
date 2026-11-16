//! Plex-flavored XML emission.
//!
//! Response types are declared once in their JSON form (see `container`), and XML is derived
//! from that by walking the serde data model. This works because the two formats are
//! isomorphic in Plex's usage: a scalar is an attribute, and a nested value is a child
//! element named after its key.
//!
//! Deriving XML from the JSON shape rather than declaring both avoids `quick-xml`'s `@`
//! attribute prefix leaking into JSON keys, and keeps 60-odd entity types from needing a
//! hand-maintained second declaration.

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("a MediaContainer must serialize to a JSON object, got {0}")]
    NotAnObject(&'static str),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

/// Serializes `value` as Plex would, under a root element named `root`.
///
/// # Errors
///
/// Fails if `value` does not serialize to a JSON object, or if serialization itself fails.
pub fn to_plex_xml<T: Serialize>(value: &T, root: &str) -> Result<String, XmlError> {
    let serialized = serde_json::to_value(value)?;
    let Value::Object(fields) = serialized else {
        return Err(XmlError::NotAnObject(kind_of(&serialized)));
    };

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    // The root keeps the long form even with no children: a real server emits
    // `<MediaContainer …>\n</MediaContainer>` for an empty container, never a self-closing tag.
    write_element(&mut out, root, &fields, true);
    Ok(out)
}

fn write_element(out: &mut String, name: &str, fields: &Map<String, Value>, long_form: bool) {
    out.push('<');
    out.push_str(name);
    for (key, value) in fields {
        if let Some(rendered) = scalar(value) {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            escape_into(out, &rendered);
            out.push('"');
        }
    }

    let children: Vec<(&String, &Value)> = fields
        .iter()
        .filter(|(_, value)| scalar(value).is_none())
        .collect();

    if children.is_empty() && !long_form {
        out.push_str(" />\n");
        return;
    }

    out.push_str(">\n");
    for (key, value) in children {
        write_child(out, key, value);
    }
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
}

fn write_child(out: &mut String, key: &str, value: &Value) {
    match value {
        // An item's `Extras` keeps the long form when empty, as measured on a film and a
        // series with none.
        Value::Object(fields) => {
            write_element(out, element_name(key, fields), fields, key == "Extras");
        }
        Value::Array(items) => {
            for item in items {
                if let Value::Object(fields) = item {
                    // An item keeps the long form even with no children, as the root does:
                    // measured on a series and a playlist with nothing inside, on an empty
                    // shelf, on the filterless `Meta` type of `/categories`, and on a
                    // collection's tree.
                    let long_form = matches!(key, "Metadata" | "Hub" | "Type" | "MetadataItem");
                    write_element(out, element_name(key, fields), fields, long_form);
                }
            }
        }
        _ => {}
    }
}

/// The XML element name for a child, which is its JSON key except under `Metadata`.
///
/// The one place the two formats are not isomorphic. JSON puts every item of a listing in a
/// single `Metadata` array; XML names each element after what the item is, so a film is
/// `<Video>` and a series `<Directory>` inside the same response. Verified against the
/// reference server on both a movie and a show library.
///
/// An unrecognized type falls back to `Directory`, which is what Plex uses for everything
/// that is browsed into rather than played.
fn element_name<'a>(key: &'a str, fields: &Map<String, Value>) -> &'a str {
    if key != "Metadata" {
        return key;
    }
    match fields.get("type").and_then(Value::as_str) {
        Some("movie" | "episode" | "clip") => "Video",
        Some("track") => "Track",
        Some("photo") => "Photo",
        Some("playlist") => "Playlist",
        Some("common") => "Common",
        _ => "Directory",
    }
}

/// Renders a scalar as Plex writes it in an attribute, or `None` for nested values.
///
/// Booleans become `1`/`0`: XML has no boolean, and Plex picked the numeric form. The JSON
/// side keeps whatever type the model declares, which is why the same field can be `true` in
/// JSON and `"1"` in XML.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::Bool(true) => Some("1".to_owned()),
        Value::Bool(false) => Some("0".to_owned()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        // `null` is indistinguishable from an absent attribute, and Plex omits absent ones.
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// Escapes XML metacharacters, and non-ASCII as numeric character references.
///
/// The numeric references are not a safety measure but a fidelity one: a real server emits
/// `Ann&#233;e` despite declaring `encoding="UTF-8"`, so byte-exact fixture comparison
/// requires reproducing it. The apostrophe is escaped for the same reason — attributes are
/// double-quoted, so it needs none, yet the reference writes `Homer&#39;s Odyssey`.
///
/// Line breaks are escaped for a reason beyond fidelity: a parser normalizes a raw newline
/// inside an attribute to a space, so a synopsis written raw loses its paragraphs. The
/// reference writes `\r\n` as `&#xD;&#xA;`, in hex unlike the other references.
fn escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            '\t' => out.push_str("&#x9;"),
            c if c.is_ascii() => out.push(c),
            c => {
                out.push_str("&#");
                out.push_str(&u32::from(c).to_string());
                out.push(';');
            }
        }
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use serde::Serialize;

    use super::{escape_into, to_plex_xml, XmlError};

    #[derive(Serialize)]
    struct Leaf {
        id: i64,
        path: String,
    }

    #[derive(Serialize)]
    struct Container {
        size: u32,
        #[serde(rename = "allowSync")]
        allow_sync: bool,
        title1: String,
        #[serde(rename = "Location", skip_serializing_if = "Vec::is_empty")]
        location: Vec<Leaf>,
    }

    fn container(location: Vec<Leaf>) -> Container {
        Container {
            size: 10,
            allow_sync: false,
            title1: "Plex Library".to_owned(),
            location,
        }
    }

    #[test]
    fn reproduces_the_reference_layout() {
        // Shape taken from a captured /library/sections response: declaration on its own
        // line, no indentation, one element per line, leaf elements self-closed with a
        // space before the slash.
        let rendered = to_plex_xml(
            &container(vec![Leaf {
                id: 17,
                path: "/mnt/movies".to_owned(),
            }]),
            "MediaContainer",
        )
        .expect("serializes");

        assert_eq!(
            rendered,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <MediaContainer size=\"10\" allowSync=\"0\" title1=\"Plex Library\">\n\
             <Location id=\"17\" path=\"/mnt/movies\" />\n\
             </MediaContainer>\n"
        );
    }

    #[test]
    fn encodes_booleans_as_digits_not_keywords() {
        let rendered = to_plex_xml(&container(vec![]), "MediaContainer").expect("serializes");
        assert!(rendered.contains("allowSync=\"0\""));
        assert!(!rendered.contains("false"));
    }

    #[test]
    fn keeps_the_root_open_even_with_no_children() {
        // A real server emits `<MediaContainer …>\n</MediaContainer>` for an empty
        // container rather than a self-closing tag; /identity is the canonical example.
        let rendered = to_plex_xml(&container(vec![]), "MediaContainer").expect("serializes");
        assert!(rendered.ends_with(">\n</MediaContainer>\n"));
        assert!(!rendered.contains("/>"));
    }

    #[test]
    fn preserves_declaration_order_rather_than_sorting() {
        let rendered = to_plex_xml(&container(vec![]), "MediaContainer").expect("serializes");
        let size = rendered.find("size=").expect("size present");
        let allow_sync = rendered.find("allowSync=").expect("allowSync present");
        let title = rendered.find("title1=").expect("title1 present");
        assert!(size < allow_sync && allow_sync < title);
    }

    #[test]
    fn escapes_non_ascii_as_numeric_references() {
        // Matches the reference server, which emits `Ann&#233;e` despite declaring UTF-8.
        let rendered = to_plex_xml(
            &container(vec![Leaf {
                id: 1,
                path: "Année".to_owned(),
            }]),
            "MediaContainer",
        )
        .expect("serializes");
        assert!(rendered.contains("path=\"Ann&#233;e\""));
    }

    #[test]
    fn escapes_attribute_metacharacters() {
        let rendered = to_plex_xml(
            &container(vec![Leaf {
                id: 1,
                path: "a&b<c>\"d\"".to_owned(),
            }]),
            "MediaContainer",
        )
        .expect("serializes");
        assert!(rendered.contains("path=\"a&amp;b&lt;c&gt;&quot;d&quot;\""));
    }

    #[test]
    fn omits_skipped_collections_entirely() {
        let rendered = to_plex_xml(&container(vec![]), "MediaContainer").expect("serializes");
        assert!(!rendered.contains("Location"));
    }

    #[derive(Serialize)]
    struct Listing {
        size: u32,
        #[serde(rename = "Metadata")]
        metadata: Vec<Item>,
    }

    #[derive(Serialize)]
    struct Item {
        #[serde(rename = "ratingKey")]
        rating_key: String,
        #[serde(rename = "type")]
        kind: &'static str,
    }

    #[test]
    fn names_listing_elements_after_the_item_type() {
        // A film is <Video> and a series <Directory>, in the same response — the one rule
        // that cannot be derived from the JSON shape.
        let rendered = to_plex_xml(
            &Listing {
                size: 2,
                metadata: vec![
                    Item {
                        rating_key: "1".to_owned(),
                        kind: "movie",
                    },
                    Item {
                        rating_key: "2".to_owned(),
                        kind: "show",
                    },
                ],
            },
            "MediaContainer",
        )
        .expect("serializes");

        // Never self-closed, even with nothing inside: captured on a series with no tags.
        assert!(
            rendered.contains(
                "<Video ratingKey=\"1\" type=\"movie\">
</Video>"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "<Directory ratingKey=\"2\" type=\"show\">
</Directory>"
            ),
            "{rendered}"
        );
        assert!(!rendered.contains("<Metadata"), "{rendered}");
    }

    #[test]
    fn escapes_an_apostrophe_as_the_reference_does() {
        let mut out = String::new();
        escape_into(&mut out, "Homer's Odyssey");
        assert_eq!(out, "Homer&#39;s Odyssey");
    }

    #[test]
    fn escapes_line_breaks_so_a_parser_keeps_them() {
        // Captured on a synopsis: `…attives.&#xD;&#xA;https://…`.
        let mut out = String::new();
        escape_into(&mut out, "attives.\r\nhttps");
        assert_eq!(out, "attives.&#xD;&#xA;https");
    }

    #[test]
    fn rejects_a_non_object_root() {
        let error = to_plex_xml(&vec![1, 2, 3], "MediaContainer").expect_err("not an object");
        assert!(matches!(error, XmlError::NotAnObject("an array")));
    }
}
