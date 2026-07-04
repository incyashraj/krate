use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let rustc = command_output("rustc", &["-V"]).unwrap_or_else(|| "unknown".to_owned());
    let git_sha = command_output("git", &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=KRATE_RUSTC_VERSION={rustc}");
    println!("cargo:rustc-env=KRATE_GIT_SHA={git_sha}");
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
