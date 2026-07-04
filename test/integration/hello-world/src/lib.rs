#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        bindings::krate::phase1::host::print("Hello, Krate!");
        bindings::krate::phase1::host::exit(0);
    }
}

bindings::export!(Component with_types_in bindings);
