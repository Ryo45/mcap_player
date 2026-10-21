use std::{
    fs,
    path::{Path, PathBuf},
};
use viewer_core::{Bookmark, BookmarkDocument, SourceFingerprint};

#[derive(Default)]
pub(crate) struct BookmarkState {
    document: Option<BookmarkDocument>,
    warning: Option<String>,
}

impl BookmarkState {
    pub(crate) fn load_for_source(&mut self, source_path: &Path, source: &SourceFingerprint) {
        self.document = None;
        self.warning = None;
        let path = bookmark_path(source_path);
        if !path.exists() {
            self.document = BookmarkDocument::new(source.clone(), Vec::new()).ok();
            return;
        }
        match fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|json| BookmarkDocument::from_json(&json).map_err(|error| error.to_string()))
        {
            Ok(document) if document.source() == source => {
                let mut bookmarks = document.bookmarks().to_vec();
                bookmarks.sort_by_key(Bookmark::time);
                self.document = BookmarkDocument::new(source.clone(), bookmarks).ok();
            }
            Ok(document) => {
                self.warning = Some(format!(
                    "Bookmarks ignored: source fingerprint {}:{} does not match",
                    document.source().algorithm(),
                    document.source().value()
                ));
                self.document = BookmarkDocument::new(source.clone(), Vec::new()).ok();
            }
            Err(error) => {
                self.warning = Some(format!("Bookmarks ignored: {error}"));
                self.document = BookmarkDocument::new(source.clone(), Vec::new()).ok();
            }
        }
    }

    pub(crate) fn bookmarks(&self) -> &[Bookmark] {
        self.document
            .as_ref()
            .map_or(&[], |document| document.bookmarks())
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
}

fn bookmark_path(source_path: &Path) -> PathBuf {
    let directory = if source_path.is_dir() {
        source_path
    } else {
        source_path.parent().unwrap_or_else(|| Path::new("."))
    };
    directory.join("bookmarks.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, BookmarkId};

    #[test]
    fn directory_sources_use_their_own_bookmarks_sidecar() {
        let directory =
            std::env::temp_dir().join(format!("mcap-directory-bookmarks-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let source = SourceFingerprint::new("test", "split-recording").unwrap();
        let bookmark =
            Bookmark::point(BookmarkId::new("one").unwrap(), ArrivalTime(10), "A", None).unwrap();
        let document = BookmarkDocument::new(source.clone(), vec![bookmark]).unwrap();
        fs::write(
            directory.join("bookmarks.json"),
            document.to_json_pretty().unwrap(),
        )
        .unwrap();
        let mut state = BookmarkState::default();
        state.load_for_source(&directory, &source);
        assert_eq!(state.bookmarks().len(), 1);
        assert!(state.warning().is_none());
        assert_eq!(
            bookmark_path(&directory.join("part.mcap")),
            directory.join("bookmarks.json")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bookmarks_are_exposed_in_time_order_from_valid_documents() {
        let source = SourceFingerprint::new("test", "source").unwrap();
        let mut bookmarks = vec![
            Bookmark::point(
                BookmarkId::new("later").unwrap(),
                ArrivalTime(20),
                "Later",
                None,
            )
            .unwrap(),
            Bookmark::point(
                BookmarkId::new("first").unwrap(),
                ArrivalTime(10),
                "First",
                None,
            )
            .unwrap(),
        ];
        bookmarks.sort_by_key(Bookmark::time);
        let document = BookmarkDocument::new(source, bookmarks).unwrap();
        let state = BookmarkState {
            document: Some(document),
            warning: None,
        };
        assert_eq!(state.bookmarks()[0].time(), ArrivalTime(10));
    }
}
