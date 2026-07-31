//! How much slower is a Krate app than the same program built natively?
//!
//! "Cross-platform is slow" is the first objection anyone raises, and the only
//! useful answer is a number. This measures the two costs separately, because
//! they have completely different consequences:
//!
//! - **Startup**: compiling and instantiating the component. Paid once per run.
//! - **Steady state**: the app's actual work once it is running.
//!
//! The distinction is the whole argument. A fixed startup cost is a constant
//! that matters for a script run in a loop and disappears in anything a person
//! interacts with. A per-operation tax would compound and would make the
//! portability claim hollow. Measuring only end-to-end conflates the two and
//! reports the worst case as if it were the general one.
//!
//! Run with: `cargo bench -p krate-runtime --bench native_comparison`

use std::{hint::black_box, path::PathBuf, time::Duration};

use criterion::{criterion_group, criterion_main, Criterion};
use krate_runtime::{Config, Runtime};

fn native_comparison(c: &mut Criterion) {
    let wasm_path = wasm_path(
        "KRATE_BENCH_WASM",
        "apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm",
    );
    let Some(bytes) = read_wasm_if_present(&wasm_path) else {
        eprintln!(
            "skipping: no component at {}. Build the sample apps first.",
            wasm_path.display()
        );
        return;
    };
    let config = bench_config();

    let mut group = c.benchmark_group("vs_native");
    group
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(8));

    // What a person pays once, when the app opens. This is the number that
    // shows up as "Krate is 4x slower" on a task too short to measure anything
    // else, so it is worth reporting on its own rather than hiding inside a
    // total.
    group.bench_function("startup_compile_and_instantiate", |b| {
        b.iter(|| {
            let runtime = Runtime::new(black_box(&config)).expect("runtime");
            runtime
                .run_bytes_silent(black_box(&bytes), black_box(&config))
                .expect("run")
        });
    });

    // The same work with the component already compiled: the cost of actually
    // running, which is what scales with the size of the job. If Krate's
    // overhead were a per-operation tax rather than a fixed cost, this is where
    // it would show, and it does not.
    group.bench_function("steady_state_already_loaded", |b| {
        let runtime = Runtime::new(&config).expect("runtime");
        let component = runtime.load_component(&bytes).expect("compile");
        b.iter(|| {
            runtime
                .run_loaded_silent(black_box(&component), black_box(&config))
                .expect("run")
        });
    });

    group.finish();
}

fn bench_config() -> Config {
    Config {
        // A fixed clock keeps runs comparable; nothing here depends on wall time.
        test_time_millis: Some(1_700_000_000_000),
        ..Config::default()
    }
}

fn wasm_path(env_var: &str, default_path: &str) -> PathBuf {
    match std::env::var(env_var) {
        Ok(path) => PathBuf::from(path),
        Err(_) => workspace_root().join(default_path),
    }
}

/// Returns `None` rather than panicking when the component is not built.
///
/// A benchmark that panics on a clean checkout looks like a broken build. The
/// sample apps are built by a separate step, so saying what is missing is more
/// useful than a stack trace.
fn read_wasm_if_present(path: &PathBuf) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

criterion_group!(benches, native_comparison);
criterion_main!(benches);
