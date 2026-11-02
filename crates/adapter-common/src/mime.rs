//! The media type Krate judges a file by, from its name.
//!
//! One table for every host that hands an app a file the person dropped on
//! it: the desktop's drop (`ui.dropzone:<mime>` is checked against this) and
//! a browser tab's, which must answer the same for the same file. A browser's
//! own `File.type` is not used: it differs by browser and by system, and the
//! grant must not.

/// The media type for a file named `name`, by its extension (any case).
/// Anything not listed is `application/octet-stream`.
pub fn mime_for_file_name(name: &str) -> &'static str {
    let ext = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    }
    .to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "log" | "md" | "csv" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_judged_by_its_extension_in_any_case() {
        assert_eq!(mime_for_file_name("notes.TXT"), "text/plain");
        assert_eq!(mime_for_file_name("photo.final.jpeg"), "image/jpeg");
        assert_eq!(
            mime_for_file_name("archive.tar.gz"),
            "application/octet-stream"
        );
        // A leading dot is a hidden file's name, not an extension, as
        // std::path::Path::extension reads it.
        assert_eq!(mime_for_file_name(".txt"), "application/octet-stream");
        assert_eq!(mime_for_file_name("README"), "application/octet-stream");
        assert_eq!(mime_for_file_name(""), "application/octet-stream");
    }
}
