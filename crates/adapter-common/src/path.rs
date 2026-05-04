//! Shared filesystem path helpers for host adapters.

use std::path::PathBuf;

/// A normalized logical path from a Layer36 app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalPath {
    normalized: String,
}

impl LogicalPath {
    /// Validate and normalize a path string before a host adapter passes it to
    /// native filesystem APIs.
    ///
    /// This helper keeps Phase 2 intentionally conservative: UTF-8 is already
    /// guaranteed by WIT, `\` is treated as a portable separator, `.` and
    /// duplicate separators are removed, and `..` is rejected before adapter
    /// code decides how to resolve the path against its sandbox root.
    pub fn parse(input: &str) -> Result<Self, PathError> {
        if input.is_empty() {
            return Err(PathError::Empty);
        }

        if input.chars().any(|ch| ch == '\0' || ch.is_control()) {
            return Err(PathError::ControlCharacter);
        }

        let portable = input.replace('\\', "/");
        let is_absolute = portable.starts_with('/');
        let mut parts = Vec::new();

        for part in portable.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(PathError::ParentTraversal),
                part => parts.push(part),
            }
        }

        let normalized = if parts.is_empty() {
            if is_absolute {
                "/".to_string()
            } else {
                ".".to_string()
            }
        } else {
            let mut normalized = parts.join("/");
            if is_absolute {
                normalized.insert(0, '/');
            }
            normalized
        };

        Ok(Self { normalized })
    }

    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.normalized)
    }
}

/// Errors returned by shared path helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path contains a control character")]
    ControlCharacter,
    #[error("path contains parent traversal")]
    ParentTraversal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_paths() {
        let path = LogicalPath::parse("./fixtures//public\\note.txt").expect("valid path");

        assert_eq!(path.as_str(), "fixtures/public/note.txt");
        assert_eq!(
            path.to_path_buf(),
            PathBuf::from("fixtures/public/note.txt")
        );
    }

    #[test]
    fn keeps_absolute_paths_absolute() {
        let path = LogicalPath::parse("/tmp//layer36/file.txt").expect("valid path");

        assert_eq!(path.as_str(), "/tmp/layer36/file.txt");
    }

    #[test]
    fn normalizes_root_and_current_directory_paths() {
        assert_eq!(LogicalPath::parse("/").expect("root path").as_str(), "/");
        assert_eq!(LogicalPath::parse(".").expect("current dir").as_str(), ".");
        assert_eq!(LogicalPath::parse("./").expect("current dir").as_str(), ".");
    }

    #[test]
    fn rejects_empty_paths() {
        assert_eq!(LogicalPath::parse("").unwrap_err(), PathError::Empty);
    }

    #[test]
    fn rejects_control_characters_and_parent_traversal() {
        assert_eq!(
            LogicalPath::parse("notes/\0today.txt").unwrap_err(),
            PathError::ControlCharacter
        );
        assert_eq!(
            LogicalPath::parse("notes/../secret.txt").unwrap_err(),
            PathError::ParentTraversal
        );
    }
}
