use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let source = PathBuf::from("../../Plan/founder-os/tasks.json");
    println!("cargo:rerun-if-changed={}", source.display());

    let bytes = fs::read(&source).expect("read Plan/founder-os/tasks.json");
    let root: Value = serde_json::from_slice(&bytes).expect("parse tasks.json");
    let root_goal = root
        .get("root_goal")
        .and_then(Value::as_str)
        .expect("root_goal string");
    let tasks = root
        .get("tasks")
        .and_then(Value::as_array)
        .expect("tasks array");

    let mut generated = String::new();
    generated.push_str("const ROOT_GOAL: &str = ");
    generated.push_str(&format!("{:?};\n", root_goal));
    generated.push_str("const SEED_TASKS: &[SeedTask] = &[\n");

    for task in tasks {
        let text = |key: &str| {
            task.get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{key} string"))
        };
        let status = match text("status") {
            "PARKED" => 0,
            "BLOCKED" => 1,
            "READY" => 2,
            "LOCKED" => 3,
            "WAITING" => 4,
            "DONE" => 5,
            "DROPPED" => 6,
            other => panic!("unknown status {other}"),
        };
        let lane = match text("focus_lane") {
            "build" => 0,
            "company" => 1,
            other => panic!("unknown focus lane {other}"),
        };
        let deps = task
            .get("depends_on")
            .and_then(Value::as_array)
            .expect("depends_on array");

        generated.push_str("    SeedTask { id: ");
        generated.push_str(&format!("{:?}", text("id")));
        generated.push_str(", title: ");
        generated.push_str(&format!("{:?}", text("title")));
        generated.push_str(", outcome: ");
        generated.push_str(&format!("{:?}", text("outcome")));
        generated.push_str(", category: ");
        generated.push_str(&format!("{:?}", text("category")));
        generated.push_str(&format!(", lane: {lane}, status: {status}, deps: &["));
        for dep in deps {
            let dep = dep.as_str().expect("dependency id string");
            generated.push_str(&format!("{:?},", dep));
        }
        generated.push_str("] },\n");
    }

    generated.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("tasks_seed.rs"), generated).expect("write generated task seed");
}
