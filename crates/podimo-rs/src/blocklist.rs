//! Loads the `.block-list` file. Lines starting with `#` are comments; only
//! the first whitespace-separated token of each line is used. Matching is
//! substring against the full request URL (not equality).

use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub(crate) struct BlockList(HashSet<String>);

impl BlockList {
    pub(crate) fn load<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();
        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };

        let mut entries = HashSet::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(token) = trimmed.split_whitespace().next() {
                entries.insert(token.to_string());
            }
        }
        Self(entries)
    }

    pub(crate) fn contains_substring(&self, url: &str) -> bool {
        self.0.iter().any(|token| url.contains(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn missing_path_returns_empty_set() {
        let bl = BlockList::load("/does/not/exist");
        assert!(!bl.contains_substring("anything"));
        assert!(bl.0.is_empty());
    }

    #[test]
    fn empty_file_returns_empty_set() {
        let f = NamedTempFile::new().unwrap();
        let bl = BlockList::load(f.path());
        assert!(bl.0.is_empty());
    }

    #[test]
    fn parses_ids_strips_comments_blank_lines_and_indented_comments() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            "# a leading comment\n\
             \n\
             1234567890\n\
             \x20\x20abcdefghij\x20\x20\n\
             \n\
             # another comment\n\
             de9b2081-9fc5-489f-b9d3-d744ed9cab20 # inline description\n\
             \x20\x20\x20\x20# indented comment line\n\
             "
        )
        .unwrap();
        let bl = BlockList::load(f.path());
        assert!(bl.contains_substring("https://x/1234567890"));
        assert!(bl.contains_substring("https://x/abcdefghij"));
        assert!(bl.contains_substring("https://x/de9b2081-9fc5-489f-b9d3-d744ed9cab20"));
        // Comment text must NOT be matchable as a token.
        assert!(!bl.contains_substring("https://x/inline"));
        assert!(!bl.contains_substring("https://x/indented"));
    }

    #[test]
    fn only_first_whitespace_token_is_kept() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "ABCDE description goes here").unwrap();
        let bl = BlockList::load(f.path());
        assert!(bl.contains_substring("https://x/ABCDE"));
        // The trailing tail is not stored as a token.
        assert!(!bl.contains_substring("https://x/description"));
    }
}
