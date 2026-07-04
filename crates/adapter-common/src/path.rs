//! Shared filesystem path helpers for host adapters.

use std::path::PathBuf;

const MAX_LOGICAL_PATH_BYTES: usize = 4096;
const MAX_PATH_SEGMENT_BYTES: usize = 255;

/// A normalized logical path from a Krate app.
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
        if input.contains(':') {
            return Err(PathError::UnsupportedPrefix);
        }

        let portable = input.replace('\\', "/");
        if portable.len() > MAX_LOGICAL_PATH_BYTES {
            return Err(PathError::PathTooLong);
        }
        let is_absolute = portable.starts_with('/');
        let mut parts = Vec::new();

        for part in portable.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(PathError::ParentTraversal),
                part => {
                    if part.len() > MAX_PATH_SEGMENT_BYTES {
                        return Err(PathError::SegmentTooLong);
                    }
                    if is_reserved_windows_segment(part) {
                        return Err(PathError::ReservedName);
                    }
                    if has_windows_ambiguous_suffix(part) {
                        return Err(PathError::AmbiguousWindowsSuffix);
                    }
                    parts.push(part)
                }
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
        if normalized.len() > MAX_LOGICAL_PATH_BYTES {
            return Err(PathError::PathTooLong);
        }

        Ok(Self { normalized })
    }

    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.normalized)
    }

    pub fn is_root_like(&self) -> bool {
        matches!(self.normalized.as_str(), "." | "/")
    }
}

/// Filesystem operation shape used before host adapters touch native paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsOperation {
    Existing,
    CreateLeaf,
    RemoveLeaf,
    RenameSource,
    RenameDestination,
}

impl FsOperation {
    pub fn allows_missing_leaf(self) -> bool {
        matches!(self, Self::CreateLeaf | Self::RenameDestination)
    }

    pub fn validate_target(self, path: &LogicalPath) -> Result<(), PathError> {
        if path.is_root_like()
            && matches!(
                self,
                Self::RemoveLeaf | Self::RenameSource | Self::RenameDestination
            )
        {
            return Err(PathError::UnsafeRootOperation);
        }

        Ok(())
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
    #[error("path segment exceeds maximum length")]
    SegmentTooLong,
    #[error("path exceeds maximum length")]
    PathTooLong,
    #[error("path segment ends with a trailing space or dot")]
    AmbiguousWindowsSuffix,
    #[error("path targets a reserved device-style name")]
    ReservedName,
    #[error("path uses an unsupported prefix or separator form")]
    UnsupportedPrefix,
    #[error("operation cannot target the filesystem root")]
    UnsafeRootOperation,
}

fn is_reserved_windows_segment(segment: &str) -> bool {
    let normalized = segment
        .trim_end_matches([' ', '.'])
        .split('.')
        .next()
        .unwrap_or(segment);
    if normalized.is_empty() {
        return false;
    }

    let upper = normalized.to_ascii_uppercase();
    if !upper.is_ascii() {
        return false;
    }

    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        name if name.len() == 4 => match name.as_bytes() {
            [b'C', b'O', b'M', digit] | [b'L', b'P', b'T', digit] => {
                matches!(digit, b'1'..=b'9')
            }
            _ => false,
        },
        _ => false,
    }
}

fn has_windows_ambiguous_suffix(segment: &str) -> bool {
    segment.ends_with(' ') || segment.ends_with('.')
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
        let path = LogicalPath::parse("/tmp//krate/file.txt").expect("valid path");

        assert_eq!(path.as_str(), "/tmp/krate/file.txt");
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

    #[test]
    fn rejects_windows_style_prefix_forms() {
        assert_eq!(
            LogicalPath::parse("C:/Users/yash/Documents/note.txt").unwrap_err(),
            PathError::UnsupportedPrefix
        );
        assert_eq!(
            LogicalPath::parse("report.txt:secret").unwrap_err(),
            PathError::UnsupportedPrefix
        );
    }

    #[test]
    fn rejects_oversized_path_segment_and_total_length() {
        let long_segment = format!("tmp/{}", "a".repeat(MAX_PATH_SEGMENT_BYTES + 1));
        assert_eq!(
            LogicalPath::parse(&long_segment).unwrap_err(),
            PathError::SegmentTooLong
        );

        let long_path = "a".repeat(MAX_LOGICAL_PATH_BYTES + 1);
        assert_eq!(
            LogicalPath::parse(&long_path).unwrap_err(),
            PathError::PathTooLong
        );
    }

    #[test]
    fn rejects_windows_reserved_device_names() {
        assert_eq!(
            LogicalPath::parse("logs/con.txt").unwrap_err(),
            PathError::ReservedName
        );
        assert_eq!(
            LogicalPath::parse("tmp/COM1").unwrap_err(),
            PathError::ReservedName
        );
        assert_eq!(
            LogicalPath::parse("tmp/lpt9.log").unwrap_err(),
            PathError::ReservedName
        );
        assert_eq!(
            LogicalPath::parse("tmp/nul. ").unwrap_err(),
            PathError::ReservedName
        );
        assert!(LogicalPath::parse("tmp/config.txt").is_ok());
    }

    #[test]
    fn rejects_windows_ambiguous_trailing_segment_suffixes() {
        assert_eq!(
            LogicalPath::parse("logs/report.").unwrap_err(),
            PathError::AmbiguousWindowsSuffix
        );
        assert_eq!(
            LogicalPath::parse("logs/report ").unwrap_err(),
            PathError::AmbiguousWindowsSuffix
        );
        assert_eq!(
            LogicalPath::parse("logs/archive./item").unwrap_err(),
            PathError::AmbiguousWindowsSuffix
        );
        assert!(LogicalPath::parse("logs/report.v1").is_ok());
    }

    #[test]
    fn parse_does_not_panic_on_non_ascii_four_byte_segments() {
        let input = String::from_utf8(vec![b'2', b'-', 0xCC, 0x8B]).expect("valid utf8");
        let result = LogicalPath::parse(&input);
        assert!(result.is_ok(), "unexpected parse result: {result:?}");
    }

    #[test]
    fn operation_intents_report_missing_leaf_rules() {
        assert!(!FsOperation::Existing.allows_missing_leaf());
        assert!(FsOperation::CreateLeaf.allows_missing_leaf());
        assert!(!FsOperation::RemoveLeaf.allows_missing_leaf());
        assert!(!FsOperation::RenameSource.allows_missing_leaf());
        assert!(FsOperation::RenameDestination.allows_missing_leaf());
    }

    #[test]
    fn destructive_operations_reject_root_like_targets() {
        let current = LogicalPath::parse(".").expect("current path");
        let root = LogicalPath::parse("/").expect("root path");

        assert_eq!(
            FsOperation::RemoveLeaf.validate_target(&current),
            Err(PathError::UnsafeRootOperation)
        );
        assert_eq!(
            FsOperation::RenameSource.validate_target(&root),
            Err(PathError::UnsafeRootOperation)
        );
        assert!(FsOperation::Existing.validate_target(&current).is_ok());
        assert!(FsOperation::CreateLeaf.validate_target(&current).is_ok());
    }
}
