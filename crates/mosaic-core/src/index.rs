use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroize;

/// Index of virtual files within the vault.
/// Stored in VaultHeader.file_index, serialized via bincode.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FileIndex {
    pub entries: BTreeMap<String, FileEntry>,
    /// Explicitly created directories (empty dirs would otherwise vanish).
    pub directories: BTreeSet<String>,
}

/// Legacy format without directories field — used for migration.
#[derive(Deserialize)]
pub(crate) struct FileIndexLegacy {
    pub entries: BTreeMap<String, FileEntry>,
}

impl Zeroize for FileIndex {
    fn zeroize(&mut self) {
        self.entries.clear();
        self.directories.clear();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    pub size: u64,
    pub created_at: u64,
    pub modified_at: u64,
    /// A file can span multiple tiles
    pub segments: Vec<FileSegment>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileSegment {
    pub pool_id: u32,
    pub offset: u64,
    pub length: u64,
}

impl FileIndex {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            directories: BTreeSet::new(),
        }
    }

    pub(crate) fn from_legacy(legacy: FileIndexLegacy) -> Self {
        Self {
            entries: legacy.entries,
            directories: BTreeSet::new(),
        }
    }

    pub fn insert(&mut self, path: &str, entry: FileEntry) {
        let normalized = normalize_path(path);
        self.entries.insert(normalized, entry);
    }

    pub fn get(&self, path: &str) -> Option<&FileEntry> {
        let normalized = normalize_path(path);
        self.entries.get(&normalized)
    }

    pub fn get_mut(&mut self, path: &str) -> Option<&mut FileEntry> {
        let normalized = normalize_path(path);
        self.entries.get_mut(&normalized)
    }

    pub fn remove(&mut self, path: &str) -> Option<FileEntry> {
        let normalized = normalize_path(path);
        self.entries.remove(&normalized)
    }

    pub fn add_dir(&mut self, path: &str) {
        let normalized = normalize_path(path);
        if !normalized.is_empty() {
            self.directories.insert(normalized);
        }
    }

    pub fn remove_dir(&mut self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.directories.remove(&normalized)
    }

    /// Lists entries directly under `dir`.
    /// Returns file/directory names (not full paths).
    pub fn list_dir(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() || dir == "/" {
            String::new()
        } else {
            let mut d = normalize_path(dir);
            if !d.ends_with('/') {
                d.push('/');
            }
            d
        };

        let mut names = BTreeSet::new();

        // Files
        for key in self.entries.keys() {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if rest.is_empty() {
                    continue;
                }
                if let Some(slash_pos) = rest.find('/') {
                    names.insert(rest[..slash_pos].to_string());
                } else {
                    names.insert(rest.to_string());
                }
            }
        }

        // Explicit directories
        for key in &self.directories {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if rest.is_empty() {
                    continue;
                }
                if let Some(slash_pos) = rest.find('/') {
                    names.insert(rest[..slash_pos].to_string());
                } else {
                    names.insert(rest.to_string());
                }
            }
            // Also include top-level dirs when listing root
            if prefix.is_empty() && !key.contains('/') {
                names.insert(key.clone());
            }
        }

        names.into_iter().collect()
    }

    pub fn contains(&self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.entries.contains_key(&normalized)
    }

    /// Checks if a path acts as a directory (has entries beneath it or is explicitly created).
    pub fn is_dir(&self, dir: &str) -> bool {
        if dir.is_empty() || dir == "/" {
            return true;
        }
        let normalized = normalize_path(dir);
        // Check explicit directories
        if self.directories.contains(&normalized) {
            return true;
        }
        // Check implicit directories (have files beneath them)
        let prefix = {
            let mut d = normalized;
            if !d.ends_with('/') {
                d.push('/');
            }
            d
        };
        self.entries.keys().any(|k| k.starts_with(&prefix))
    }
}

/// Normalizes a virtual path: strips leading/trailing slashes.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(size: u64) -> FileEntry {
        FileEntry {
            size,
            created_at: 1700000000,
            modified_at: 1700000000,
            segments: vec![FileSegment {
                pool_id: 0,
                offset: 0,
                length: size,
            }],
        }
    }

    #[test]
    fn test_file_index_crud() {
        let mut idx = FileIndex::new();

        // Insert
        idx.insert("photos/cat.jpg", make_entry(4200000));
        idx.insert("photos/dog.jpg", make_entry(3100000));
        idx.insert("docs/notes.txt", make_entry(12000));

        // Get
        assert!(idx.get("photos/cat.jpg").is_some());
        assert_eq!(idx.get("photos/cat.jpg").unwrap().size, 4200000);
        assert!(idx.get("nonexistent.txt").is_none());

        // List dir
        let root = idx.list_dir("/");
        assert_eq!(root, vec!["docs", "photos"]);

        let photos = idx.list_dir("photos");
        assert_eq!(photos, vec!["cat.jpg", "dog.jpg"]);

        let docs = idx.list_dir("docs");
        assert_eq!(docs, vec!["notes.txt"]);

        // Remove
        let removed = idx.remove("photos/dog.jpg");
        assert!(removed.is_some());
        assert!(idx.get("photos/dog.jpg").is_none());

        let photos_after = idx.list_dir("photos");
        assert_eq!(photos_after, vec!["cat.jpg"]);
    }

    #[test]
    fn test_is_dir() {
        let mut idx = FileIndex::new();
        idx.insert("a/b/c.txt", make_entry(100));

        assert!(idx.is_dir("/"));
        assert!(idx.is_dir("a"));
        assert!(idx.is_dir("a/b"));
        assert!(!idx.is_dir("a/b/c.txt"));
        assert!(!idx.is_dir("nonexistent"));
    }

    #[test]
    fn test_normalize_paths() {
        let mut idx = FileIndex::new();
        idx.insert("/photos/cat.jpg", make_entry(100));
        assert!(idx.get("photos/cat.jpg").is_some());
        assert!(idx.get("/photos/cat.jpg").is_some());
    }

    #[test]
    fn test_list_empty_dir() {
        let idx = FileIndex::new();
        assert!(idx.list_dir("/").is_empty());
    }
}
