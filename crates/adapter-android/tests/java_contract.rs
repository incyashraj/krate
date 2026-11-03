//! Every call Krate's Rust makes into the player activity names a method
//! that exists in its Java, with the same JNI signature.
//!
//! The two halves are only joined at run time on a phone: a misspelt method
//! or one wrong letter of a signature compiles on both sides, then fails as a
//! NoSuchMethodError that `java::with_activity` logs and turns into "no
//! answer" -- the feature is simply dead on every phone, and nothing on a
//! Mac says so. This reads both sides as text and holds them together.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn read_all(dir: &Path, ext: &str) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).expect("source folder") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push_str(&std::fs::read_to_string(&path).expect("source file"));
            out.push('\n');
        }
    }
    out
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Each `"krateName"` the Rust names, with the signature literal that follows
/// it when there is one.
fn rust_calls(source: &str) -> Vec<(String, Option<String>)> {
    let mut calls = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    while let Some(at) = source[i..].find("\"krate") {
        let start = i + at + 1;
        let end = start
            + source[start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(0);
        i = end;
        let name = &source[start..end];
        // A method of the activity: krate then a capital (not "krate-..."
        // strings or "krate_player").
        if bytes.get(end) != Some(&b'"') || !name[5..].starts_with(|c: char| c.is_ascii_uppercase())
        {
            continue;
        }
        let rest = source[end + 1..].trim_start();
        let signature = rest
            .strip_prefix(',')
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix("\"("))
            .and_then(|rest| rest.find('"').map(|close| format!("({}", &rest[..close])));
        calls.push((name.to_string(), signature));
    }
    calls
}

fn jni_type(java: &str) -> String {
    let mut ty = java.trim();
    let mut dims = 0;
    while let Some(inner) = ty.strip_suffix("[]") {
        ty = inner.trim_end();
        dims += 1;
    }
    let base = match ty {
        "boolean" => "Z",
        "int" => "I",
        "long" => "J",
        "float" => "F",
        "double" => "D",
        "void" => "V",
        "byte" => "B",
        "String" => "Ljava/lang/String;",
        "Class<?>" => "Ljava/lang/Class;",
        other => panic!("a Java type this check does not know: {other}"),
    };
    format!("{}{base}", "[".repeat(dims))
}

/// Each public `krate*` method of the activity, with its JNI signature.
fn java_methods(source: &str) -> BTreeMap<String, Vec<String>> {
    let mut methods: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut i = 0;
    while let Some(at) = source[i..].find("public ") {
        let head = i + at + "public ".len();
        i = head;
        let Some(open) = source[head..].find('(') else {
            break;
        };
        let words: Vec<&str> = source[head..head + open].split_whitespace().collect();
        let Some((name, before)) = words.split_last() else {
            continue;
        };
        if !name.starts_with("krate") || before.is_empty() || before.len() > 2 {
            continue;
        }
        let ret = before.last().expect("a return type");
        let close = source[head + open..].find(')').expect("parameters close");
        let params = &source[head + open + 1..head + open + close];
        let mut signature = String::from("(");
        for param in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let param = param.strip_prefix("final ").unwrap_or(param);
            let (ty, _name) = param.rsplit_once(' ').expect("a typed parameter");
            signature.push_str(&jni_type(ty));
        }
        signature.push(')');
        signature.push_str(&jni_type(ret));
        methods.entry(name.to_string()).or_default().push(signature);
    }
    methods
}

#[test]
fn every_activity_method_rust_calls_exists_in_java_with_that_signature() {
    let dir = crate_dir();
    let rust =
        read_all(&dir.join("src"), "rs") + &read_all(&dir.join("../player-android/src"), "rs");
    let java = read_all(
        &dir.join("../player-android/android/java/tech/krate/player"),
        "java",
    );
    let calls = rust_calls(&rust);
    let methods = java_methods(&java);
    // The check sees what it should: dozens of calls, the keyboard's among
    // them, each with the signature it is called by.
    assert!(calls.len() > 60, "found only {} calls", calls.len());
    assert!(calls
        .iter()
        .any(|(name, sig)| name == "krateTextEntry" && sig.as_deref() == Some("(ZZ)Z")));
    let mut wrong = Vec::new();
    for (name, signature) in &calls {
        match methods.get(name) {
            None => wrong.push(format!("{name}: no such method in the activity")),
            Some(found) => {
                if let Some(signature) = signature {
                    if !found.contains(signature) {
                        wrong.push(format!(
                            "{name}: Rust calls {signature}, Java has {found:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "Rust and Java disagree:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn the_check_reads_signatures_the_way_jni_spells_them() {
    let java = "public String[] krateA(final String a, int b) {}\n\
                public static byte[][] krateB(long x, float[] y) {}\n\
                public void krateC() {}";
    let methods = java_methods(java);
    assert_eq!(
        methods["krateA"],
        vec!["(Ljava/lang/String;I)[Ljava/lang/String;"]
    );
    assert_eq!(methods["krateB"], vec!["(J[F)[[B"]);
    assert_eq!(methods["krateC"], vec!["()V"]);
    let rust = "env.call_method(activity, \"krateA\", \"(Z)V\", &[]);\nask(\"krateB\");";
    assert_eq!(
        rust_calls(rust),
        vec![
            ("krateA".to_string(), Some("(Z)V".to_string())),
            ("krateB".to_string(), None)
        ]
    );
}
