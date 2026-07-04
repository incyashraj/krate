use std::{hint::black_box, path::PathBuf, time::Duration};

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use krate_policy::SessionPolicy;
use krate_runtime::{Config, Runtime};
use wasmtime::{component::Component, Config as WasmtimeConfig, Engine};

const PRINT_LOOP_CALLS: u64 = 1_000;
const PHASE2_SMOKE_INPUT: &str = "phase2-smoke-input.txt";

fn phase1_runtime_benches(c: &mut Criterion) {
    let hello_wasm = wasm_path(
        "KRATE_HELLO_WASM",
        "test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm",
    );
    let print_loop_wasm = wasm_path(
        "KRATE_PRINT_LOOP_WASM",
        "test/integration/print-loop/target/wasm32-wasip1/release/print_loop.wasm",
    );

    let hello = read_wasm(&hello_wasm);
    let print_loop = read_wasm(&print_loop_wasm);
    let runtime_config = Config::default();

    let mut group = c.benchmark_group("phase1_runtime");
    group
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10));

    group.bench_function("engine_construction", |b| {
        b.iter(|| Runtime::new(black_box(&runtime_config)).expect("runtime should initialize"));
    });

    group.bench_function("component_from_binary_hello", |b| {
        let engine = component_engine();
        b.iter(|| {
            Component::from_binary(black_box(&engine), black_box(&hello))
                .expect("hello component should compile")
        });
    });

    group.bench_function("cold_start_to_main_hello", |b| {
        b.iter(|| {
            let runtime =
                Runtime::new(black_box(&runtime_config)).expect("runtime should initialize");
            runtime
                .run_bytes_silent(black_box(&hello), black_box(&runtime_config))
                .expect("hello component should run")
        });
    });

    group.bench_function("first_print_latency_hello", |b| {
        let runtime = Runtime::new(&runtime_config).expect("runtime should initialize");
        let component = runtime
            .load_component(&hello)
            .expect("hello component should compile");
        b.iter(|| {
            runtime
                .run_loaded_silent(black_box(&component), black_box(&runtime_config))
                .expect("hello component should run")
        });
    });

    group.throughput(Throughput::Elements(PRINT_LOOP_CALLS));
    group.bench_function("per_call_print_dispatch_1000", |b| {
        let runtime = Runtime::new(&runtime_config).expect("runtime should initialize");
        let component = runtime
            .load_component(&print_loop)
            .expect("print-loop component should compile");
        b.iter(|| {
            runtime
                .run_loaded_silent(black_box(&component), black_box(&runtime_config))
                .expect("print-loop component should run")
        });
    });

    group.finish();
}

fn phase2_runtime_benches(c: &mut Criterion) {
    let phase2_smoke_wasm = wasm_path(
        "KRATE_PHASE2_SMOKE_WASM",
        "test/integration/phase2-smoke/target/wasm32-wasip1/release/phase2_smoke.wasm",
    );
    let clock_wasm = wasm_path(
        "KRATE_CLOCK_WASM",
        "apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm",
    );

    let phase2_smoke = read_wasm(&phase2_smoke_wasm);
    let clock = read_wasm(&clock_wasm);
    let phase2_smoke_config = phase2_smoke_config();
    let clock_config = clock_config();
    let input_path = ensure_phase2_smoke_input();

    let mut group = c.benchmark_group("phase2_runtime");
    group
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10));

    group.bench_function("component_from_binary_phase2_smoke", |b| {
        let engine = component_engine();
        b.iter(|| {
            Component::from_binary(black_box(&engine), black_box(&phase2_smoke))
                .expect("Phase 2 smoke component should compile")
        });
    });

    group.bench_function("cold_start_to_main_phase2_smoke", |b| {
        b.iter(|| {
            let runtime =
                Runtime::new(black_box(&phase2_smoke_config)).expect("runtime should initialize");
            runtime
                .run_bytes_silent(black_box(&phase2_smoke), black_box(&phase2_smoke_config))
                .expect("Phase 2 smoke component should run")
        });
    });

    group.bench_function("loaded_run_phase2_smoke", |b| {
        let runtime = Runtime::new(&phase2_smoke_config).expect("runtime should initialize");
        let component = runtime
            .load_component(&phase2_smoke)
            .expect("Phase 2 smoke component should compile");
        b.iter(|| {
            runtime
                .run_loaded_silent(black_box(&component), black_box(&phase2_smoke_config))
                .expect("Phase 2 smoke component should run")
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
    restore_phase2_smoke_input(input_path);
}

fn component_engine() -> Engine {
    let mut config = WasmtimeConfig::new();
    config.wasm_component_model(true);
    Engine::new(&config).expect("engine should initialize")
}

fn phase2_smoke_config() -> Config {
    Config {
        session_policy: SessionPolicy::from_cli_grants(&[format!("fs.read:{PHASE2_SMOKE_INPUT}")])
            .expect("Phase 2 smoke grant should parse"),
        ..Config::default()
    }
}

fn clock_config() -> Config {
    Config {
        test_time_millis: Some(1_234_567_890),
        ..Config::default()
    }
}

fn ensure_phase2_smoke_input() -> Option<Vec<u8>> {
    let path = PathBuf::from(PHASE2_SMOKE_INPUT);
    let previous = std::fs::read(&path).ok();

    std::fs::write(path, b"Krate Phase 2 input\n")
        .expect("Phase 2 smoke benchmark input should be writable");

    previous
}

fn restore_phase2_smoke_input(previous: Option<Vec<u8>>) {
    let path = PathBuf::from(PHASE2_SMOKE_INPUT);

    match previous {
        Some(bytes) => {
            std::fs::write(path, bytes).expect("Phase 2 smoke benchmark input should restore");
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
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

criterion_group!(benches, phase1_runtime_benches, phase2_runtime_benches);
criterion_main!(benches);
