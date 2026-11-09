//! Typed identifiers.
//!
//! The Plex API carries a dozen distinct numeric ids. Keeping them in separate newtypes is
//! what makes passing a section id where an item id belongs a compile error rather than a
//! silent lookup against the wrong table.

macro_rules! numeric_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);

        impl $name {
            pub const fn new(value: i64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

numeric_id! {
    /// Identifies someone who can be given access to libraries.
    UserId
}

numeric_id! {
    /// Identifies a library section (what the UI calls a library).
    SectionId
}

numeric_id! {
    /// Identifies a metadata item: a movie, show, season, episode, album, track.
    RatingKey
}

numeric_id! {
    /// Identifies one encoding of an item.
    MediaId
}

numeric_id! {
    /// Identifies one file of an encoding.
    PartId
}

numeric_id! {
    /// Identifies one track inside a file.
    StreamId
}

numeric_id! {
    /// Identifies a tag: one genre, one country, or one person in one credit role.
    TagId
}

numeric_id! {
    /// Identifies a person, whatever roles they are credited in.
    PersonId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_of_different_kinds_are_distinct_types() {
        // The real assertion is that this file compiles: `SectionId` and `RatingKey` share a
        // representation but cannot be substituted for one another.
        let section = SectionId::new(4);
        let item = RatingKey::new(4);
        assert_eq!(section.get(), item.get());
    }

    #[test]
    fn displays_as_the_bare_number() {
        assert_eq!(SectionId::new(17).to_string(), "17");
    }
}
