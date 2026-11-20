//! Jellyfin's identifiers, as a wire encoding of our global ids.
//!
//! Jellyfin names everything by a GUID, written as 32 hexadecimal digits — or with dashes, or
//! in upper case, which it reads alike. Ours are Relay global ids, `Type:key`; a Jellyfin id
//! carries both, so the binding reads the global id back without looking anything up:
//!
//! ```text
//! tttttttt 00000000 kkkkkkkkkkkkkkkk
//! type     reserved key, as 64 bits
//! ```

use klapz_api::GlobalId;

/// The types a Jellyfin id can name, by the number it carries. Numbers are never reused:
/// clients keep ids across sessions.
const TYPES: &[(u32, &str)] = &[
    (1, klapz_api::server::SERVER_TYPE),
    (2, klapz_api::access::USER_TYPE),
    (3, klapz_api::library::LIBRARY_TYPE),
    (4, klapz_api::item::ITEM_TYPE),
    (5, klapz_api::media::VERSION_TYPE),
    (6, klapz_api::media::FILE_TYPE),
    (7, klapz_api::media::STREAM_TYPE),
    (8, klapz_api::tag::TAG_TYPE),
    (9, klapz_api::tag::PERSON_TYPE),
    (10, klapz_api::collection::COLLECTION_TYPE),
    (11, klapz_api::playlist::PLAYLIST_TYPE),
    (12, klapz_api::extra::EXTRA_TYPE),
    (13, klapz_api::playback::PLAYBACK_TYPE),
    (14, klapz_api::playlist::PLAYLIST_ENTRY_TYPE),
    (15, crate::browse::STUDIO_TYPE),
    (16, crate::browse::YEAR_TYPE),
    (17, klapz_api::marker::MARKER_TYPE),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// Not a GUID at all: what the reference answers `400`.
    #[error("not a GUID")]
    Malformed,
    /// A GUID, but none of ours: `404`, as for an id the reference does not know.
    #[error("not an id this server gave out")]
    Unknown,
}

/// The Jellyfin id of a global id, as a type and a key.
pub fn of(type_name: &str, key: i64) -> Option<String> {
    let code = TYPES
        .iter()
        .find(|(_, name)| *name == type_name)
        .map(|(code, _)| *code)?;
    Some(format!(
        "{code:08x}00000000{:016x}",
        u64::from_be_bytes(key.to_be_bytes())
    ))
}

/// The Jellyfin id of a Relay global id, as the official API publishes it.
pub fn of_global(global: &str) -> Option<String> {
    let decoded = GlobalId::decode(global).ok()?;
    of(&decoded.type_name, decoded.key)
}

/// The global id a Jellyfin id carries.
///
/// # Errors
///
/// [`IdError::Malformed`] for what is no GUID, [`IdError::Unknown`] for a GUID this server
/// never gave out.
pub fn decode(jellyfin: &str) -> Result<GlobalId, IdError> {
    let digits: String = jellyfin.chars().filter(|&c| c != '-').collect();
    let dashes = jellyfin.len() - digits.len();
    if digits.len() != 32 || !(dashes == 0 || dashes == 4) {
        return Err(IdError::Malformed);
    }
    let value = u128::from_str_radix(&digits, 16).map_err(|_| IdError::Malformed)?;
    let code = u32::try_from(value >> 96).map_err(|_| IdError::Malformed)?;
    if (value >> 64) & 0xffff_ffff != 0 {
        return Err(IdError::Unknown);
    }
    let type_name = TYPES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
        .ok_or(IdError::Unknown)?;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the low 64 bits are the key, kept whole"
    )]
    let key = i64::from_be_bytes((value as u64).to_be_bytes());
    Ok(GlobalId::new(type_name, key))
}

/// The global id a Jellyfin id carries, encoded as the official API takes it, when it names
/// a thing of this type.
///
/// # Errors
///
/// As [`decode`]; a well-formed id of another type is [`IdError::Unknown`].
pub fn global_of_type(jellyfin: &str, type_name: &str) -> Result<String, IdError> {
    let global = decode(jellyfin)?;
    if global.type_name != type_name {
        return Err(IdError::Unknown);
    }
    Ok(GlobalId::encode(&global.type_name, global.key))
}

/// The same id with dashes, as Jellyfin writes it in `UserData.Key` and in some URLs.
pub fn dashed(id: &str) -> String {
    if id.len() != 32 {
        return id.to_owned();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn an_item_id_is_its_type_and_key_in_32_digits() {
        let id = of("Item", 577_986).unwrap();
        assert_eq!(id, "0000000400000000000000000008d1c2");
        assert_eq!(decode(&id).unwrap(), GlobalId::new("Item", 577_986));
    }

    #[test]
    fn every_type_round_trips_through_its_global_id() {
        for (_, type_name) in TYPES {
            for key in [0, 1, 42, i64::MAX, -1] {
                let global = GlobalId::encode(type_name, key);
                let id = of_global(&global).unwrap();
                assert_eq!(id.len(), 32);
                assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
                assert_eq!(decode(&id).unwrap(), GlobalId::new(*type_name, key));
            }
        }
    }

    #[test]
    fn two_types_sharing_a_key_have_two_ids() {
        assert_ne!(of("Item", 4), of("Library", 4));
    }

    #[test]
    fn a_dashed_or_upper_case_id_reads_alike() {
        let id = of("Library", 3).unwrap();
        assert_eq!(decode(&dashed(&id)).unwrap(), GlobalId::new("Library", 3));
        assert_eq!(
            decode(&id.to_ascii_uppercase()).unwrap(),
            GlobalId::new("Library", 3)
        );
    }

    #[test]
    fn what_is_no_guid_is_malformed() {
        for text in [
            "zzz",
            "",
            "00000004000000000000000000000001ff",
            "0000000400000000000000000000000g",
            "0000-0004000000000000000000000001",
        ] {
            assert_eq!(decode(text), Err(IdError::Malformed), "{text}");
        }
    }

    #[test]
    fn a_guid_this_server_never_gave_out_is_unknown() {
        // The reference's own ids, and one of ours with its reserved digits set.
        for text in [
            "c4927ec38c9f3d59490edf05de2d8a2a",
            "00000000000000000000000000000001",
            "000000040000000100000000000000ff",
        ] {
            assert_eq!(decode(text), Err(IdError::Unknown), "{text}");
        }
    }

    #[test]
    fn an_id_of_another_type_is_unknown_where_a_type_is_expected() {
        let library = of("Library", 3).unwrap();
        assert_eq!(global_of_type(&library, "Item"), Err(IdError::Unknown));
        assert_eq!(
            global_of_type(&library, "Library").unwrap(),
            GlobalId::encode("Library", 3)
        );
    }

    #[test]
    fn an_unknown_type_name_has_no_id() {
        assert_eq!(of("Nothing", 1), None);
        assert_eq!(of_global("not base64!"), None);
    }

    #[test]
    fn dashes_go_where_jellyfin_writes_them() {
        assert_eq!(
            dashed("db4c1708cbb5dd1676284a40f2950aba"),
            "db4c1708-cbb5-dd16-7628-4a40f2950aba"
        );
    }
}
