#![no_main]

use krate_adapter_common::path::{FsOperation, LogicalPath};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        if let Ok(path) = LogicalPath::parse(input) {
            let _ = path.as_str();
            let _ = path.to_path_buf();
            let _ = FsOperation::Existing.validate_target(&path);
            let _ = FsOperation::CreateLeaf.validate_target(&path);
            let _ = FsOperation::RemoveLeaf.validate_target(&path);
            let _ = FsOperation::RenameSource.validate_target(&path);
            let _ = FsOperation::RenameDestination.validate_target(&path);
        }
    }
});
