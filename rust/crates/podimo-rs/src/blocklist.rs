//! Loads the `.block-list` file. Lines starting with `#` are comments; only
//! the first whitespace-separated token of each line is used. Matching is
//! substring against the full request URL (not equality).

use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct BlockList(HashSet<String>);

impl BlockList {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
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

    pub fn contains_substring(&self, url: &str) -> bool {
        self.0.iter().any(|token| url.contains(token))
    }

    #[cfg(test)]
    pub fn from_tokens<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn loads_tokens_and_ignores_comments() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "# header\n1234567890\nabcdefghij\n\n# commented\n  uuid-token  trailing\n"
        )
        .unwrap();
        let bl = BlockList::load(f.path());
        assert!(bl.contains_substring("https://x/feed/1234567890.xml"));
        assert!(bl.contains_substring("https://x/feed/abcdefghij.xml"));
        assert!(bl.contains_substring("https://x/feed/uuid-token.xml"));
        assert!(!bl.contains_substring("https://x/feed/other.xml"));
    }

    #[test]
    fn missing_file_yields_empty() {
        let bl = BlockList::load("/does/not/exist");
        assert!(!bl.contains_substring("anything"));
    }
}
