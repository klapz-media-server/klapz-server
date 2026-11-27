//! Server settings: what an owner tunes while the server runs.
//!
//! Every setting is declared here, in [`DEFINITIONS`], with its kind and its default; storage
//! keeps only the values an owner chose. A key is dotted lower-case words, the machinery it
//! drives first (`transcoder.h264.preset`), and a key not declared is refused, never stored.
//! What the process needs before it can read its database — where to listen, the database
//! itself, secrets, directories — stays in the environment.

use crate::ids::UserId;

/// The longest key: a namespace and a few words.
pub const MAX_KEY_LENGTH: usize = 96;

/// The longest value an owner may send, whatever the kind: a value is a word or a number.
pub const MAX_VALUE_LENGTH: usize = 256;

/// The most settings one change names: a ceiling on what one request makes the server check.
pub const MAX_UPDATES: usize = 100;

/// What values a setting takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// One of these words, written as listed.
    Choice(&'static [&'static str]),
    /// A whole number between these bounds, both included.
    Integer { minimum: i64, maximum: i64 },
    /// `true` or `false`.
    Boolean,
    /// An address clients can be sent to, as a pairing takes one: `http` or `https`, a host,
    /// perhaps a port, nothing after. Empty for none.
    Address,
}

/// A setting the server knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingDefinition {
    pub key: &'static str,
    pub kind: SettingKind,
    /// The value in force while no owner chose one, in its canonical form.
    pub default: &'static str,
    pub description: &'static str,
}

/// x264's presets, fastest first: each trades encoding speed for compression.
pub const H264_PRESETS: &[&str] = &[
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
];

pub const H264_PRESET: SettingDefinition = SettingDefinition {
    key: "transcoder.h264.preset",
    kind: SettingKind::Choice(H264_PRESETS),
    // The reference's, for every conversion it was captured making.
    default: "veryfast",
    description: "How hard x264 works on every video the server converts to H.264. A slower \
                  preset makes a smaller or sharper picture at the same bitrate and costs more \
                  processor time; too slow a one cannot keep up with playback. Applies from the \
                  next stream.",
};

pub const PUBLIC_URL: SettingDefinition = SettingDefinition {
    key: "general.public_url",
    kind: SettingKind::Address,
    default: "",
    description: "The address the server is reached at from outside its network, such as a                   domain or a tunnel leading to it: `https://media.example.com`. Empty when the                   server is only reached at the addresses it listens on.",
};

/// Every setting, by key.
pub const DEFINITIONS: &[SettingDefinition] = &[PUBLIC_URL, H264_PRESET];

/// The setting declared under this key.
#[must_use]
pub fn definition(key: &str) -> Option<&'static SettingDefinition> {
    DEFINITIONS.iter().find(|definition| definition.key == key)
}

/// Whether `key` is written as a key must be: at least two words separated by dots, each of
/// lower-case ASCII letters, digits and `_`, starting with a letter, `MAX_KEY_LENGTH` at most.
#[must_use]
pub fn is_valid_key(key: &str) -> bool {
    let word = |word: &str| {
        word.starts_with(|c: char| c.is_ascii_lowercase())
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    key.len() <= MAX_KEY_LENGTH && key.split('.').count() >= 2 && key.split('.').all(word)
}

/// Whether `key` lies in `namespace`: is it, or starts with it and a dot.
#[must_use]
pub fn in_namespace(key: &str, namespace: &str) -> bool {
    key.strip_prefix(namespace)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// Why a value was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingError {
    /// No setting has this key.
    UnknownSetting,
    /// The value is not one this setting takes.
    InvalidValue,
    /// The same key came twice in one change.
    RepeatedSetting,
}

impl SettingDefinition {
    /// The value in its canonical form, if this setting takes it: a choice as listed, a number
    /// without sign or leading zeros, a boolean in lower case. Surrounding blanks are ignored.
    ///
    /// # Errors
    ///
    /// `InvalidValue` when the setting does not take it.
    pub fn check(&self, value: &str) -> Result<String, SettingError> {
        let value = value.trim();
        if value.len() > MAX_VALUE_LENGTH {
            return Err(SettingError::InvalidValue);
        }
        match self.kind {
            SettingKind::Choice(choices) => choices
                .iter()
                .find(|choice| **choice == value)
                .map(|choice| (*choice).to_owned()),
            SettingKind::Integer { minimum, maximum } => value
                .parse::<i64>()
                .ok()
                .filter(|number| (minimum..=maximum).contains(number))
                .map(|number| number.to_string()),
            SettingKind::Boolean => match value {
                "true" | "false" => Some(value.to_owned()),
                _ => None,
            },
            // Lower case, as a pairing's address is stored; a closing slash is not "after".
            SettingKind::Address => {
                let address = value.strip_suffix('/').unwrap_or(value);
                (address.is_empty() || crate::access::is_connection_uri(address))
                    .then(|| address.to_ascii_lowercase())
            }
        }
        .ok_or(SettingError::InvalidValue)
    }
}

/// A value an owner chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSetting {
    pub key: String,
    pub value: String,
    pub updated_at: i64,
    /// Who chose it. A user deleted since is still named here; the API finds no one.
    pub updated_by: Option<UserId>,
}

/// A setting with the value an owner chose for it, if one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub definition: &'static SettingDefinition,
    pub stored: Option<StoredSetting>,
}

impl Setting {
    /// The value in force. A stored value the setting no longer takes, such as a choice a later
    /// version dropped, gives way to the default rather than reaching the machinery.
    #[must_use]
    pub fn value(&self) -> String {
        self.stored
            .as_ref()
            .and_then(|stored| self.definition.check(&stored.value).ok())
            .unwrap_or_else(|| self.definition.default.to_owned())
    }
}

/// One change to the stored settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingChange {
    /// Keep this value, already checked.
    Set { key: &'static str, value: String },
    /// Forget the value, back to the default.
    Reset { key: &'static str },
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait SettingsRepository: Send + Sync + std::fmt::Debug {
    /// The stored values of these keys. A key no owner set is absent.
    async fn stored(&self, keys: &[&str]) -> Result<Vec<StoredSetting>, SettingsError>;

    /// Applies every change, or none.
    async fn apply(
        &self,
        changes: &[SettingChange],
        by: UserId,
        at: i64,
    ) -> Result<(), SettingsError>;
}

/// The settings under `namespace`, or all of them, by key.
///
/// # Errors
///
/// When storage fails.
pub async fn settings(
    repository: &dyn SettingsRepository,
    namespace: Option<&str>,
) -> Result<Vec<Setting>, SettingsError> {
    let definitions: Vec<&'static SettingDefinition> = DEFINITIONS
        .iter()
        .filter(|definition| {
            namespace.is_none_or(|namespace| in_namespace(definition.key, namespace))
        })
        .collect();
    with_stored(repository, definitions).await
}

/// The value in force for one setting.
///
/// # Errors
///
/// When storage fails.
pub async fn value_of(
    repository: &dyn SettingsRepository,
    definition: &'static SettingDefinition,
) -> Result<String, SettingsError> {
    let stored = repository
        .stored(&[definition.key])
        .await?
        .into_iter()
        .next();
    Ok(Setting { definition, stored }.value())
}

async fn with_stored(
    repository: &dyn SettingsRepository,
    definitions: Vec<&'static SettingDefinition>,
) -> Result<Vec<Setting>, SettingsError> {
    if definitions.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<&str> = definitions
        .iter()
        .map(|definition| definition.key)
        .collect();
    let mut stored = repository.stored(&keys).await?;
    Ok(definitions
        .into_iter()
        .map(|definition| Setting {
            definition,
            stored: stored
                .iter()
                .position(|row| row.key == definition.key)
                .map(|at| stored.swap_remove(at)),
        })
        .collect())
}

/// What an owner asked of one setting: a value, or `None` for the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingUpdate {
    pub key: String,
    pub value: Option<String>,
}

/// One asked change refused, by its place in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejected {
    pub index: usize,
    pub error: SettingError,
}

/// Checks every update, then applies them all at once; one refused, none is applied. The
/// settings named, as they now are, in the order asked.
///
/// # Errors
///
/// When storage fails.
pub async fn update_settings(
    repository: &dyn SettingsRepository,
    updates: &[SettingUpdate],
    by: UserId,
    at: i64,
) -> Result<Result<Vec<Setting>, Vec<Rejected>>, SettingsError> {
    let mut changes = Vec::with_capacity(updates.len());
    let mut rejected = Vec::new();
    for (index, update) in updates.iter().enumerate() {
        let checked = definition(&update.key)
            .ok_or(SettingError::UnknownSetting)
            .and_then(|definition| {
                if updates[..index]
                    .iter()
                    .any(|before| before.key == update.key)
                {
                    return Err(SettingError::RepeatedSetting);
                }
                Ok(match &update.value {
                    Some(value) => SettingChange::Set {
                        key: definition.key,
                        value: definition.check(value)?,
                    },
                    None => SettingChange::Reset {
                        key: definition.key,
                    },
                })
            });
        match checked {
            Ok(change) => changes.push(change),
            Err(error) => rejected.push(Rejected { index, error }),
        }
    }
    if !rejected.is_empty() {
        return Ok(Err(rejected));
    }
    repository.apply(&changes, by, at).await?;
    let named = changes
        .iter()
        .filter_map(|change| match change {
            SettingChange::Set { key, .. } | SettingChange::Reset { key } => definition(key),
        })
        .collect();
    Ok(Ok(with_stored(repository, named).await?))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testing::InMemorySettings;

    #[test]
    fn every_definition_is_well_formed() {
        for (index, definition) in DEFINITIONS.iter().enumerate() {
            assert!(is_valid_key(definition.key), "{}", definition.key);
            assert_eq!(
                definition.check(definition.default).as_deref(),
                Ok(definition.default),
                "{} defaults to a value it refuses",
                definition.key
            );
            assert!(!definition.description.is_empty(), "{}", definition.key);
            if let Some(next) = DEFINITIONS.get(index + 1) {
                assert!(definition.key < next.key, "{} out of order", next.key);
            }
        }
    }

    #[test]
    fn keys_are_dotted_lower_case_words() {
        for key in ["transcoder.h264.preset", "scanner.deep_analysis", "a.b2"] {
            assert!(is_valid_key(key), "{key}");
        }
        for key in [
            "transcoder",
            "Transcoder.preset",
            "transcoder..preset",
            ".transcoder",
            "transcoder.",
            "transcoder.2pass",
            "transcoder.h264-preset",
            "transcoder.h264 preset",
            "",
        ] {
            assert!(!is_valid_key(key), "{key}");
        }
        assert!(!is_valid_key(&format!("a.{}", "b".repeat(MAX_KEY_LENGTH))));
    }

    #[test]
    fn a_namespace_holds_whole_words() {
        assert!(in_namespace("transcoder.h264.preset", "transcoder"));
        assert!(in_namespace("transcoder.h264.preset", "transcoder.h264"));
        assert!(in_namespace(
            "transcoder.h264.preset",
            "transcoder.h264.preset"
        ));
        assert!(!in_namespace("transcoder.h264.preset", "transcoder.h26"));
        assert!(!in_namespace("transcoders.x", "transcoder"));
    }

    #[test]
    fn values_are_checked_by_kind_and_written_canonically() {
        let choice = H264_PRESET;
        assert_eq!(choice.check(" slow ").as_deref(), Ok("slow"));
        assert_eq!(choice.check("Slow"), Err(SettingError::InvalidValue));
        assert_eq!(choice.check("placebo"), Err(SettingError::InvalidValue));

        let integer = SettingDefinition {
            kind: SettingKind::Integer {
                minimum: 1,
                maximum: 16,
            },
            default: "2",
            ..H264_PRESET
        };
        assert_eq!(integer.check("+08").as_deref(), Ok("8"));
        assert_eq!(integer.check("16").as_deref(), Ok("16"));
        assert_eq!(integer.check("17"), Err(SettingError::InvalidValue));
        assert_eq!(integer.check("0"), Err(SettingError::InvalidValue));
        assert_eq!(integer.check("2.5"), Err(SettingError::InvalidValue));

        let boolean = SettingDefinition {
            kind: SettingKind::Boolean,
            default: "false",
            ..H264_PRESET
        };
        assert_eq!(boolean.check("true").as_deref(), Ok("true"));
        assert_eq!(boolean.check("1"), Err(SettingError::InvalidValue));
        assert_eq!(boolean.check("TRUE"), Err(SettingError::InvalidValue));

        let address = PUBLIC_URL;
        assert_eq!(
            address.check(" https://Media.Example.com/ ").as_deref(),
            Ok("https://media.example.com")
        );
        assert_eq!(
            address.check("http://192.168.1.2:32500").as_deref(),
            Ok("http://192.168.1.2:32500")
        );
        assert_eq!(address.check("").as_deref(), Ok(""));
        for refused in [
            "media.example.com",
            "ftp://media.example.com",
            "https://media.example.com/web",
            "https://media.example.com?a=1",
            "https://user@media.example.com",
            "https://media.example.com:0",
        ] {
            assert_eq!(
                address.check(refused),
                Err(SettingError::InvalidValue),
                "{refused}"
            );
        }

        assert_eq!(
            choice.check(&"s".repeat(MAX_VALUE_LENGTH + 1)),
            Err(SettingError::InvalidValue)
        );
    }

    #[test]
    fn a_stored_value_no_longer_taken_gives_way_to_the_default() {
        let setting = |value: &str| Setting {
            definition: &H264_PRESET,
            stored: Some(StoredSetting {
                key: H264_PRESET.key.to_owned(),
                value: value.to_owned(),
                updated_at: 0,
                updated_by: None,
            }),
        };
        assert_eq!(setting("slow").value(), "slow");
        assert_eq!(setting("placebo").value(), "veryfast");
        assert_eq!(
            Setting {
                definition: &H264_PRESET,
                stored: None
            }
            .value(),
            "veryfast"
        );
    }

    fn update(key: &str, value: Option<&str>) -> SettingUpdate {
        SettingUpdate {
            key: key.to_owned(),
            value: value.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn an_update_is_applied_whole_or_not_at_all() {
        let repository = InMemorySettings::default();
        let owner = UserId::new(1);

        let refused = update_settings(
            &repository,
            &[
                update(H264_PRESET.key, Some("slow")),
                update("transcoder.h265.preset", Some("slow")),
                update(H264_PRESET.key, Some("fast")),
            ],
            owner,
            100,
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(
            refused,
            [
                Rejected {
                    index: 1,
                    error: SettingError::UnknownSetting
                },
                Rejected {
                    index: 2,
                    error: SettingError::RepeatedSetting
                },
            ]
        );
        assert_eq!(
            value_of(&repository, &H264_PRESET).await.unwrap(),
            "veryfast"
        );

        let applied = update_settings(
            &repository,
            &[update(H264_PRESET.key, Some(" slow"))],
            owner,
            200,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].value(), "slow");
        let stored = applied[0].stored.as_ref().unwrap();
        assert_eq!((stored.updated_at, stored.updated_by), (200, Some(owner)));

        let reset = update_settings(&repository, &[update(H264_PRESET.key, None)], owner, 300)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reset[0].stored, None);
        assert_eq!(
            value_of(&repository, &H264_PRESET).await.unwrap(),
            "veryfast"
        );
    }

    #[tokio::test]
    async fn a_namespace_lists_its_settings_only() {
        let repository = InMemorySettings::default();
        let keys = |settings: Vec<Setting>| -> Vec<&str> {
            settings
                .iter()
                .map(|setting| setting.definition.key)
                .collect()
        };
        assert_eq!(
            keys(settings(&repository, Some("transcoder")).await.unwrap()),
            [H264_PRESET.key]
        );
        assert!(settings(&repository, Some("transcode"))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            settings(&repository, None).await.unwrap().len(),
            DEFINITIONS.len()
        );
    }
}
