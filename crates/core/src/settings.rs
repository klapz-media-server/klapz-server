//! Server settings: what an owner tunes while the server runs.
//!
//! Every setting is declared here, in [`DEFINITIONS`], with its kind and its default; storage
//! keeps only the values an owner chose. A key is dotted lower-case words, the machinery it
//! drives first (`transcoder.h264.preset`), and a key not declared is refused, never stored.
//! What the process needs before it can read its database — where to listen, the database
//! itself, secrets, directories — stays in the environment.
//!
//! A setting declared `per_library` may also be given a value for one library, which that
//! library follows instead of the server's; a library that holds none follows the server.

use crate::ids::{SectionId, UserId};

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
    /// Whether a library may hold a value of its own, which it follows instead of the server's.
    pub per_library: bool,
}

/// Where a value is kept: the server's, or one library's in place of the server's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingScope {
    Server,
    Library(SectionId),
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
    per_library: false,
};

pub const PUBLIC_URL: SettingDefinition = SettingDefinition {
    key: "general.public_url",
    kind: SettingKind::Address,
    default: "",
    description: "The address the server is reached at from outside its network, such as a \
                  domain or a tunnel leading to it: `https://media.example.com`. Empty when the \
                  server is only reached at the addresses it listens on.",
    per_library: false,
};

/// The most tasks of one kind the analysis runs at once: each a process reading a whole file,
/// often over a network share.
pub const MAX_CONCURRENT_TASKS: i64 = 16;

const fn concurrency(
    key: &'static str,
    default: &'static str,
    description: &'static str,
) -> SettingDefinition {
    SettingDefinition {
        key,
        kind: SettingKind::Integer {
            minimum: 1,
            maximum: MAX_CONCURRENT_TASKS,
        },
        default,
        description,
        per_library: false,
    }
}

pub const HEADERS_CONCURRENCY: SettingDefinition = concurrency(
    "analysis.concurrency.headers",
    "2",
    "How many files have their headers read at once, over every library: the quick analysis \
     that makes a file playable, with its name, size and hashes. Each read is a process \
     reading the start of a file.",
);

pub const DEEP_ANALYSIS_CONCURRENCY: SettingDefinition = concurrency(
    "analysis.concurrency.deep",
    "1",
    "How many files have their packets measured at once, over every library. Each is a \
     process reading a whole file.",
);

pub const SUBTITLE_EXTRACTION_CONCURRENCY: SettingDefinition = concurrency(
    "analysis.concurrency.subtitles",
    "1",
    "How many files have their subtitle tracks extracted at once, over every library. Each is \
     a process reading a whole file.",
);

pub const PREVIEW_THUMBNAILS_CONCURRENCY: SettingDefinition = concurrency(
    "analysis.concurrency.preview_thumbnails",
    "1",
    "How many files have their seeking thumbnails made at once, over every library. Each is a \
     process decoding every key frame of a video.",
);

pub const DEEP_ANALYSIS: SettingDefinition = SettingDefinition {
    key: "analysis.deep",
    kind: SettingKind::Boolean,
    default: "true",
    description: "Whether every file's packets are read once its headers are, to measure its \
                  tracks' bitrates and its duration rather than trust what the file declares. \
                  It reads the whole file. A file's headers, name, size and hashes are always \
                  read.",
    per_library: true,
};

pub const SUBTITLE_EXTRACTION: SettingDefinition = SettingDefinition {
    key: "analysis.subtitles",
    kind: SettingKind::Boolean,
    default: "true",
    description: "Whether the subtitle tracks inside every file are extracted to files of their \
                  own, kept by the file's content hash: a copy of a file already extracted is \
                  not extracted again. It reads the whole file.",
    per_library: true,
};

pub const PREVIEW_THUMBNAILS: SettingDefinition = SettingDefinition {
    key: "analysis.preview_thumbnails",
    kind: SettingKind::Boolean,
    // The reference's `GenerateBIFBehavior`, as measured: `asap`.
    default: "true",
    description: "Whether every video gets its seeking thumbnails, a picture every two seconds \
                  packed into a BIF, kept by the file's content hash: a copy of a file already \
                  done is not done again. It decodes every key frame of the video.",
    per_library: true,
};

/// Every setting, by key.
pub const DEFINITIONS: &[SettingDefinition] = &[
    DEEP_ANALYSIS_CONCURRENCY,
    HEADERS_CONCURRENCY,
    PREVIEW_THUMBNAILS_CONCURRENCY,
    SUBTITLE_EXTRACTION_CONCURRENCY,
    DEEP_ANALYSIS,
    PREVIEW_THUMBNAILS,
    SUBTITLE_EXTRACTION,
    PUBLIC_URL,
    H264_PRESET,
];

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
    /// A library was given a value of a setting that is the server's alone.
    ServerOnly,
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
    /// The values of these keys stored in `scope`. A key no owner set there is absent.
    async fn stored(
        &self,
        scope: SettingScope,
        keys: &[&str],
    ) -> Result<Vec<StoredSetting>, SettingsError>;

    /// The values of these keys that libraries hold, by library.
    async fn library_values(
        &self,
        keys: &[&str],
    ) -> Result<Vec<(SectionId, StoredSetting)>, SettingsError>;

    /// Applies every change in `scope`, or none.
    async fn apply(
        &self,
        scope: SettingScope,
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
        .stored(SettingScope::Server, &[definition.key])
        .await?
        .into_iter()
        .next();
    Ok(Setting { definition, stored }.value())
}

/// The values in force for these settings, the server's, in the order given.
///
/// # Errors
///
/// When storage fails.
pub async fn values_of(
    repository: &dyn SettingsRepository,
    definitions: &[&'static SettingDefinition],
) -> Result<Vec<String>, SettingsError> {
    Ok(with_stored(repository, definitions.to_vec())
        .await?
        .iter()
        .map(Setting::value)
        .collect())
}

/// Where a per-library boolean setting is on: the server's value, and the libraries that hold
/// the other one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    pub server: bool,
    pub except: Vec<SectionId>,
}

impl Switch {
    #[must_use]
    pub fn is_on(&self, section: SectionId) -> bool {
        self.server != self.except.contains(&section)
    }
}

/// Where each of these per-library boolean settings is on, for every library at once, in the
/// order given: what work run over every library reads before each round of it.
///
/// # Errors
///
/// When storage fails.
pub async fn switches(
    repository: &dyn SettingsRepository,
    definitions: &[&'static SettingDefinition],
) -> Result<Vec<Switch>, SettingsError> {
    let servers = with_stored(repository, definitions.to_vec()).await?;
    let libraries = repository.library_values(&keys(definitions)).await?;
    Ok(servers
        .iter()
        .map(|setting| {
            let definition = setting.definition;
            let server = setting.value() == "true";
            let except = libraries
                .iter()
                .filter(|(_, stored)| stored.key == definition.key)
                .filter(|(_, stored)| {
                    definition
                        .check(&stored.value)
                        .is_ok_and(|value| (value == "true") != server)
                })
                .map(|(section, _)| *section)
                .collect();
            Switch { server, except }
        })
        .collect())
}

/// A per-library setting as one library sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySetting {
    /// The server's, which the library follows while it holds no value of its own.
    pub server: Setting,
    /// The library's own value, if an owner chose one.
    pub stored: Option<StoredSetting>,
}

impl LibrarySetting {
    /// The value in force for the library: its own, else the server's. A value of its own the
    /// setting no longer takes gives way to the server's.
    #[must_use]
    pub fn value(&self) -> String {
        self.stored
            .as_ref()
            .and_then(|stored| self.server.definition.check(&stored.value).ok())
            .unwrap_or_else(|| self.server.value())
    }
}

/// The per-library settings under `namespace`, or all of them, as `section` sees them, by key.
///
/// # Errors
///
/// When storage fails.
pub async fn library_settings(
    repository: &dyn SettingsRepository,
    section: SectionId,
    namespace: Option<&str>,
) -> Result<Vec<LibrarySetting>, SettingsError> {
    let definitions: Vec<&'static SettingDefinition> = DEFINITIONS
        .iter()
        .filter(|definition| definition.per_library)
        .filter(|definition| {
            namespace.is_none_or(|namespace| in_namespace(definition.key, namespace))
        })
        .collect();
    of_library(repository, section, definitions).await
}

async fn of_library(
    repository: &dyn SettingsRepository,
    section: SectionId,
    definitions: Vec<&'static SettingDefinition>,
) -> Result<Vec<LibrarySetting>, SettingsError> {
    if definitions.is_empty() {
        return Ok(Vec::new());
    }
    let mut stored = repository
        .stored(SettingScope::Library(section), &keys(&definitions))
        .await?;
    Ok(with_stored(repository, definitions)
        .await?
        .into_iter()
        .map(|server| LibrarySetting {
            stored: take(&mut stored, server.definition.key),
            server,
        })
        .collect())
}

fn keys(definitions: &[&'static SettingDefinition]) -> Vec<&'static str> {
    definitions
        .iter()
        .map(|definition| definition.key)
        .collect()
}

fn take(stored: &mut Vec<StoredSetting>, key: &str) -> Option<StoredSetting> {
    stored
        .iter()
        .position(|row| row.key == key)
        .map(|at| stored.swap_remove(at))
}

async fn with_stored(
    repository: &dyn SettingsRepository,
    definitions: Vec<&'static SettingDefinition>,
) -> Result<Vec<Setting>, SettingsError> {
    if definitions.is_empty() {
        return Ok(Vec::new());
    }
    let mut stored = repository
        .stored(SettingScope::Server, &keys(&definitions))
        .await?;
    Ok(definitions
        .into_iter()
        .map(|definition| Setting {
            definition,
            stored: take(&mut stored, definition.key),
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
    let changes = match checked(updates, SettingScope::Server) {
        Ok(changes) => changes,
        Err(rejected) => return Ok(Err(rejected)),
    };
    repository
        .apply(SettingScope::Server, &changes, by, at)
        .await?;
    Ok(Ok(with_stored(repository, named(&changes)).await?))
}

/// Checks every update of a library's own values, then applies them all at once, as
/// [`update_settings`] does: a value is the library's own, `None` makes it follow the server
/// again. Only a per-library setting is taken.
///
/// # Errors
///
/// When storage fails.
pub async fn update_library_settings(
    repository: &dyn SettingsRepository,
    section: SectionId,
    updates: &[SettingUpdate],
    by: UserId,
    at: i64,
) -> Result<Result<Vec<LibrarySetting>, Vec<Rejected>>, SettingsError> {
    let scope = SettingScope::Library(section);
    let changes = match checked(updates, scope) {
        Ok(changes) => changes,
        Err(rejected) => return Ok(Err(rejected)),
    };
    repository.apply(scope, &changes, by, at).await?;
    Ok(Ok(of_library(repository, section, named(&changes)).await?))
}

fn checked(
    updates: &[SettingUpdate],
    scope: SettingScope,
) -> Result<Vec<SettingChange>, Vec<Rejected>> {
    let mut changes = Vec::with_capacity(updates.len());
    let mut rejected = Vec::new();
    for (index, update) in updates.iter().enumerate() {
        let checked = definition(&update.key)
            .ok_or(SettingError::UnknownSetting)
            .and_then(|definition| {
                if matches!(scope, SettingScope::Library(_)) && !definition.per_library {
                    return Err(SettingError::ServerOnly);
                }
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
    if rejected.is_empty() {
        Ok(changes)
    } else {
        Err(rejected)
    }
}

fn named(changes: &[SettingChange]) -> Vec<&'static SettingDefinition> {
    changes
        .iter()
        .filter_map(|change| match change {
            SettingChange::Set { key, .. } | SettingChange::Reset { key } => definition(key),
        })
        .collect()
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

    #[tokio::test]
    async fn a_library_follows_the_server_until_it_holds_a_value_of_its_own() {
        let repository = InMemorySettings::default();
        let owner = UserId::new(1);
        let (films, series) = (SectionId::new(1), SectionId::new(2));
        let deep = |section| {
            let repository = &repository;
            async move {
                library_settings(repository, section, Some(DEEP_ANALYSIS.key))
                    .await
                    .unwrap()
                    .remove(0)
                    .value()
            }
        };
        assert_eq!(deep(films).await, "true");

        update_settings(
            &repository,
            &[update(DEEP_ANALYSIS.key, Some("false"))],
            owner,
            10,
        )
        .await
        .unwrap()
        .unwrap();
        let changed = update_library_settings(
            &repository,
            series,
            &[update(DEEP_ANALYSIS.key, Some("true"))],
            owner,
            20,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(changed[0].value(), "true");
        assert_eq!(changed[0].server.value(), "false");
        assert_eq!(
            (deep(films).await, deep(series).await),
            ("false".to_owned(), "true".to_owned())
        );
        assert_eq!(
            value_of(&repository, &DEEP_ANALYSIS).await.unwrap(),
            "false",
            "the server's own value is unchanged"
        );

        let deep_analysis = switches(&repository, &[&DEEP_ANALYSIS, &SUBTITLE_EXTRACTION])
            .await
            .unwrap();
        assert!(!deep_analysis[0].is_on(films));
        assert!(deep_analysis[0].is_on(series));
        assert!(deep_analysis[1].is_on(films) && deep_analysis[1].is_on(series));

        update_library_settings(
            &repository,
            series,
            &[update(DEEP_ANALYSIS.key, None)],
            owner,
            30,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(deep(series).await, "false", "it follows the server again");
    }

    #[tokio::test]
    async fn a_library_takes_no_value_of_a_setting_that_is_the_servers_alone() {
        let repository = InMemorySettings::default();
        let refused = update_library_settings(
            &repository,
            SectionId::new(1),
            &[
                update(SUBTITLE_EXTRACTION.key, Some("false")),
                update(H264_PRESET.key, Some("slow")),
            ],
            UserId::new(1),
            10,
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(
            refused,
            [Rejected {
                index: 1,
                error: SettingError::ServerOnly
            }]
        );
        assert_eq!(
            library_settings(&repository, SectionId::new(1), None)
                .await
                .unwrap()
                .iter()
                .map(|setting| setting.server.definition.key)
                .collect::<Vec<_>>(),
            [
                DEEP_ANALYSIS.key,
                PREVIEW_THUMBNAILS.key,
                SUBTITLE_EXTRACTION.key
            ],
            "nothing was kept, and only per-library settings are listed"
        );
        assert!(library_settings(&repository, SectionId::new(1), None)
            .await
            .unwrap()
            .iter()
            .all(|setting| setting.stored.is_none()));
    }
}
