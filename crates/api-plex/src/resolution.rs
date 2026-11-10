//! Plex's resolution classes: the label a version carries, the frames `resolution=` selects,
//! and the list a client picks from.
//!
//! One ladder serves all three, measured on the reference over 99,911 versions: each class
//! is reached by either dimension, so 1920×800 is `1080` by its width and 1440×1080 by its
//! height. The three uses differ in two measured ways. A label caps the height at the
//! width, so a portrait 810×1440 is `720` rather than `1080`; the filter compares the
//! frame as it is. And `resolution=2k` is not the `2k` label but every frame 1,072 to
//! 1,440 lines tall, 1080p included: 3,416 films of a library labelled `1080`. Both rules
//! reproduce the reference exactly, on every version and every filter value.

use async_graphql::indexmap::IndexMap;
use async_graphql::{Name, Value};

/// A class, widest first: a frame reaching `width` or `height` is in it unless it reaches a
/// wider one. Below the last is `sd`.
struct Class {
    key: &'static str,
    title: &'static str,
    width: i32,
    height: i32,
}

/// The ladder as measured. 7680 is 8K's standard width: the one 8K version here is 7740
/// wide, so the threshold below it is unmeasured.
const CLASSES: [Class; 8] = [
    Class {
        key: "8k",
        title: "8K",
        width: 7680,
        height: 4320,
    },
    Class {
        key: "4k",
        title: "4K",
        width: 2880,
        height: 1540,
    },
    Class {
        key: "2.7k",
        title: "2.7K",
        width: 2802,
        height: i32::MAX,
    },
    Class {
        key: "2k",
        title: "2K",
        width: 2048,
        height: i32::MAX,
    },
    Class {
        key: "1080",
        title: "1080p",
        width: 1902,
        height: 1070,
    },
    Class {
        key: "720",
        title: "720p",
        width: 1252,
        height: 704,
    },
    Class {
        key: "576",
        title: "576p",
        width: 1016,
        height: 560,
    },
    Class {
        key: "480",
        title: "480p",
        width: 848,
        height: 462,
    },
];

const SD: &str = "sd";

/// Every key, widest first, as the reference lists them.
pub fn keys() -> impl Iterator<Item = &'static str> {
    CLASSES
        .iter()
        .map(|class| class.key)
        .chain(std::iter::once(SD))
}

pub fn title(key: &str) -> &'static str {
    CLASSES
        .iter()
        .find(|class| class.key == key)
        .map_or("SD", |class| class.title)
}

/// The label of a version: `4k`, `1080`, `sd`. None without a width.
pub fn label(width: Option<i32>, height: Option<i32>) -> Option<&'static str> {
    let width = width?;
    // A label caps the height at the width; the filter does not.
    let height = height.unwrap_or(0).min(width);
    Some(classify(width, height))
}

fn classify(width: i32, height: i32) -> &'static str {
    CLASSES
        .iter()
        .find(|class| width >= class.width || height >= class.height)
        .map_or(SD, |class| class.key)
}

/// A key the filter and the list accept. None for anything else, which the reference does
/// not offer either.
pub fn known(key: &str) -> Option<&'static str> {
    keys().find(|known| *known == key)
}

/// The frames `resolution=key` selects, as the schema's `FrameRangeInput`.
pub fn filter_range(key: &str) -> Value {
    if key == "2k" {
        // Every frame 1,072 to 1,440 lines tall that no wider class claims.
        return range(Some((i32::MAX, 1072)), Some((2802, 1441)));
    }
    label_range(key)
}

/// The frames whose label is `key`, by their dimensions as stored.
///
/// What decides whether a class is listed. The label's cap is left out — it would need the
/// smaller of two columns — so a portrait video can list a class one step off, a case the
/// reference library holds 71 times out of 99,911.
pub fn label_range(key: &str) -> Value {
    let position = CLASSES
        .iter()
        .position(|class| class.key == key)
        .unwrap_or(CLASSES.len());
    // Short of every wider class at once: narrower than the narrowest of their widths and
    // shorter than the shortest of their heights.
    let short_of = CLASSES[..position]
        .iter()
        .map(|class| (class.width, class.height))
        .reduce(|(width, height), (other_width, other_height)| {
            (width.min(other_width), height.min(other_height))
        });
    let reaching = CLASSES
        .get(position)
        .map(|class| (class.width, class.height));
    range(reaching, short_of)
}

fn range(reaching: Option<(i32, i32)>, short_of: Option<(i32, i32)>) -> Value {
    let size = |(width, height): (i32, i32)| {
        let mut fields = IndexMap::new();
        fields.insert(Name::new("width"), Value::Number(width.into()));
        fields.insert(Name::new("height"), Value::Number(height.into()));
        Value::Object(fields)
    };
    let mut fields = IndexMap::new();
    if let Some(bound) = reaching {
        fields.insert(Name::new("reaching"), size(bound));
    }
    if let Some(bound) = short_of {
        fields.insert(Name::new("shortOf"), size(bound));
    }
    Value::Object(fields)
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn labels_a_frame_by_either_dimension() {
        // Measured pairs, including every boundary the reference disagreed with the old
        // width-only rule on.
        let cases = [
            ((3840, 2160), "4k"),
            ((2880, 1200), "4k"),
            ((2802, 1440), "2.7k"),
            ((2048, 1080), "2k"),
            ((1920, 1080), "1080"),
            ((1920, 800), "1080"),
            ((1440, 1080), "1080"),
            ((1904, 1062), "1080"),
            ((1728, 1056), "720"),
            ((1892, 778), "720"),
            ((960, 720), "720"),
            ((1024, 576), "576"),
            ((720, 576), "576"),
            ((912, 384), "480"),
            ((720, 448), "sd"),
            ((720, 404), "sd"),
        ];
        for ((width, height), expected) in cases {
            assert_eq!(
                label(Some(width), Some(height)),
                Some(expected),
                "{width}×{height}"
            );
        }
    }

    #[test]
    fn a_label_caps_the_height_at_the_width() {
        assert_eq!(label(Some(810), Some(1440)), Some("720"));
        assert_eq!(label(Some(544), Some(576)), Some("480"));
        assert_eq!(label(None, Some(1080)), None);
    }

    #[test]
    fn a_label_range_is_short_of_every_wider_class() {
        let range = label_range("2k").into_json().unwrap();
        assert_eq!(range["reaching"]["width"], 2048);
        // Narrower than 2.7K's 2802 and shorter than 4K's 1540.
        assert_eq!(range["shortOf"]["width"], 2802);
        assert_eq!(range["shortOf"]["height"], 1540);
        let sd = label_range("sd").into_json().unwrap();
        assert!(sd.get("reaching").is_none());
        assert_eq!(sd["shortOf"]["width"], 848);
        let top = label_range("8k").into_json().unwrap();
        assert!(top.get("shortOf").is_none());
    }

    #[test]
    fn the_2k_filter_is_a_band_of_heights() {
        let range = filter_range("2k").into_json().unwrap();
        assert_eq!(range["reaching"]["height"], 1072);
        assert_eq!(range["shortOf"]["height"], 1441);
        assert_eq!(filter_range("1080"), label_range("1080"));
    }

    #[test]
    fn lists_every_class_widest_first() {
        assert_eq!(
            keys().collect::<Vec<_>>(),
            ["8k", "4k", "2.7k", "2k", "1080", "720", "576", "480", "sd"]
        );
        assert_eq!(title("1080"), "1080p");
        assert_eq!(title("sd"), "SD");
        assert_eq!(known("2.7k"), Some("2.7k"));
        assert_eq!(known("6k"), None);
    }
}
