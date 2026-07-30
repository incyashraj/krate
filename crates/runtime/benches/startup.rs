use std::{hint::black_box, path::PathBuf, time::Duration};

use criterion::{criterion_group, criterion_main, Criterion};
use krate_runtime::{Config, Runtime};
use wasmtime::{component::Component, Config as WasmtimeConfig, Engine};

fn phase2_runtime_benches(c: &mut Criterion) {
    let clock_wasm = wasm_path(
        "KRATE_CLOCK_WASM",
        "apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm",
    );

    let clock = read_wasm(&clock_wasm);
    let clock_config = clock_config();

    let mut group = c.benchmark_group("phase2_runtime");
    group
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10));

    group.bench_function("engine_construction", |b| {
        b.iter(|| Runtime::new(black_box(&clock_config)).expect("runtime should initialize"));
    });

    group.bench_function("component_from_binary_krate_clock", |b| {
        let engine = component_engine();
        b.iter(|| {
            Component::from_binary(black_box(&engine), black_box(&clock))
                .expect("krate-clock component should compile")
        });
    });

    group.bench_function("cold_start_to_main_krate_clock", |b| {
        b.iter(|| {
            let runtime =
                Runtime::new(black_box(&clock_config)).expect("runtime should initialize");
            runtime
                .run_bytes_silent(black_box(&clock), black_box(&clock_config))
                .expect("krate-clock component should run")
        });
    });

    group.bench_function("loaded_run_krate_clock_fixed_time", |b| {
        let runtime = Runtime::new(&clock_config).expect("runtime should initialize");
        let component = runtime
            .load_component(&clock)
            .expect("krate-clock component should compile");
        b.iter(|| {
            runtime
                .run_loaded_silent(black_box(&component), black_box(&clock_config))
                .expect("krate-clock component should run")
        });
    });

    group.finish();
}

fn component_engine() -> Engine {
    let mut config = WasmtimeConfig::new();
    config.wasm_component_model(true);
    Engine::new(&config).expect("engine should initialize")
}

fn clock_config() -> Config {
    Config {
        test_time_millis: Some(1_234_567_890),
        ..Config::default()
    }
}

fn wasm_path(env_var: &str, default_path: &str) -> PathBuf {
    std::env::var_os(env_var)
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join(default_path))
}

fn read_wasm(path: &PathBuf) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|err| {
        panic!(
            "failed to read {}: {err}. Build benchmark components first, or set the matching \
             KRATE_*_WASM environment variable.",
            path.display()
        )
    })
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("runtime crate should live under crates/runtime")
        .to_path_buf()
}

criterion_group!(benches, phase2_runtime_benches);
criterion_main!(benches);
