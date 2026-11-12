fn main() {
    // #186: stamp the commit into the build so a stale `ludex.exe` can name
    // itself. CI sets LUDEX_BUILD_COMMIT to the full SHA; a local build falls
    // back to `git rev-parse` so a hand-built copy is still identifiable.
    println!("cargo:rerun-if-env-changed=LUDEX_BUILD_COMMIT");
    println!("cargo:rustc-env=LUDEX_BUILD_COMMIT={}", build_commit());
    tauri_build::build();
}

/// Commit recorded in the binary: the CI variable, then git, then `unknown`.
fn build_commit() -> String {
    if let Ok(value) = std::env::var("LUDEX_BUILD_COMMIT") {
        let value = value.trim();
        if !value.is_empty() {
            return value.chars().take(64).collect();
        }
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if sha.is_empty() {
                "unknown".to_string()
            } else {
                sha
            }
        }
        _ => "unknown".to_string(),
    }
}
