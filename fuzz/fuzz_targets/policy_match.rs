#![no_main]

use std::str::FromStr;

use krate_manifest::Capability;
use krate_policy::SessionPolicy;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    let mut lines = input.lines();
    let Some(grant_line) = lines.next() else {
        return;
    };
    let Some(required_line) = lines.next() else {
        return;
    };

    let Ok(grant) = Capability::from_str(grant_line) else {
        return;
    };
    let Ok(required) = Capability::from_str(required_line) else {
        return;
    };

    let policy = SessionPolicy::from_grants([grant]);
    let _ = policy.allows(&required);
    let _ = policy.check(&required);
});
