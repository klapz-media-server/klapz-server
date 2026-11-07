//! Library sections, and the port through which storage supplies them.

use crate::ids::SectionId;
use crate::page::{Page, Window};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("no library section with id {0}")]
    SectionNotFound(SectionId),
    /// Storage failed.
    ///
    /// Boxed rather than stringified so the cause chain survives for the operator's logs:
    /// flattening it into a `String` is the `Other(String)` smell in disguise. Boxed rather
    /// than typed because `core` must not know which storage engine is in use.
    ///
    /// Bindings must not relay this to a client — see the Plex adapter's `ApiError`.
    #[error("library storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// The kind of media a section holds.
///
/// Plex stores this as an integer and exposes it as a string. Neither representation appears
/// here: the storage layer maps its column, and the binding maps its wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Movie,
    Show,
    Artist,
    Photo,
}

/// A filesystem root a section scans.
///
/// Carries its id because the API exposes it, and clients key on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRoot {
    pub id: i64,
    pub section_id: SectionId,
    pub path: String,
    /// When the root was added, in epoch seconds. Carried because the Plex adapter orders
    /// locations by it.
    pub created_at: i64,
}

/// A library, as the domain sees it.
///
/// Times are seconds since the Unix epoch. Keeping one unambiguous representation means
/// neither Plex's `datetime` columns nor its epoch wire format leaks into the domain.
#[derive(Debug, Clone)]
pub struct LibrarySection {
    pub id: SectionId,
    pub kind: SectionKind,
    pub title: String,
    pub title_sort: Option<String>,
    pub language: String,
    pub uuid: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub scanned_at: Option<i64>,
    /// A monotonic change counter, not a timestamp: it drives client-side cache
    /// invalidation and is unrelated to `updated_at`.
    pub content_changed_at: i64,
    /// Whether the section is excluded from normal browsing.
    pub hidden: bool,
}

/// Storage port for library data.
///
/// This trait exists to invert the dependency: `core` declares what it needs, and `db`
/// implements it, so the domain never depends on storage. It also lets `core` and the
/// bindings be tested without a database.
#[async_trait::async_trait]
pub trait LibraryRepository: Send + Sync + std::fmt::Debug {
    /// One page of sections ordered by id, without their roots.
    ///
    /// Takes a window rather than returning everything so the limit reaches SQL: this is the
    /// same port shape the metadata collections will need, where "everything" is hundreds of
    /// thousands of rows.
    async fn sections(&self, window: Window) -> Result<Page<LibrarySection>, LibraryError>;

    async fn section(&self, id: SectionId) -> Result<LibrarySection, LibraryError>;

    /// Roots for several sections in one call.
    ///
    /// Batched rather than per-section because the API resolves `roots` through a
    /// `DataLoader`: a query over fifty libraries must cost one root query, not fifty.
    /// Sections with no root are simply absent from the result.
    async fn roots_for(&self, sections: &[SectionId]) -> Result<Vec<SectionRoot>, LibraryError>;
}

/// Lets an `Arc<dyn LibraryRepository>` be used wherever a repository is expected, so the
/// bindings can hold a trait object instead of being generic over the storage type.
#[async_trait::async_trait]
impl<T: LibraryRepository + ?Sized> LibraryRepository for std::sync::Arc<T> {
    async fn sections(&self, window: Window) -> Result<Page<LibrarySection>, LibraryError> {
        (**self).sections(window).await
    }

    async fn section(&self, id: SectionId) -> Result<LibrarySection, LibraryError> {
        (**self).section(id).await
    }

    async fn roots_for(&self, sections: &[SectionId]) -> Result<Vec<SectionRoot>, LibraryError> {
        (**self).roots_for(sections).await
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::page::Window;
    use crate::testing::InMemoryLibrary;

    fn catalog() -> InMemoryLibrary {
        InMemoryLibrary::new(vec![LibrarySection {
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
    }

    #[tokio::test]
    async fn finds_a_section_by_id() {
        let section = catalog().section(SectionId::new(4)).await.expect("seeded");
        assert_eq!(section.title, "VF - Films");
    }

    #[tokio::test]
    async fn reports_the_requested_id_when_absent() {
        let error = catalog()
            .section(SectionId::new(99))
            .await
            .expect_err("absent");
        assert!(matches!(error, LibraryError::SectionNotFound(id) if id.get() == 99));
    }

    #[tokio::test]
    async fn pages_sections_through_the_window() {
        let window = Window::new(None, None, Some(1), None).expect("valid window");
        let page = catalog().sections(window).await.expect("page");
        assert_eq!(page.items.len(), 1);
        assert!(!page.has_next);
        assert!(!page.has_previous);
    }
}
