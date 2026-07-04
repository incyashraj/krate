#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        for _ in 0..1_000 {
            bindings::krate::phase1::host::print("bench");
        }

        bindings::krate::phase1::host::exit(0);
    }
}

bindings::export!(Component with_types_in bindings);
