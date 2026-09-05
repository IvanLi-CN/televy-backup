use std::{env, path::PathBuf, process::Command};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo manifest directory"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .to_path_buf();
    let mode = env::var("TELEVYBACKUP_BUILD_MODE").unwrap_or_else(|_| "development".to_string());
    let commit = env::var("TELEVYBACKUP_BUILD_COMMIT").unwrap_or_else(|_| {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&root)
                .output()
                .expect("resolve git source SHA")
                .stdout,
        )
        .expect("git source SHA is UTF-8")
        .trim()
        .to_string()
    });
    let resolver = Command::new("python3")
        .arg(root.join("scripts/product-version.py"))
        .args(["--mode", &mode, "--source-sha", &commit])
        .current_dir(&root)
        .output()
        .expect("run product version resolver");
    if !resolver.status.success() {
        panic!(
            "product version resolver failed: {}",
            String::from_utf8_lossy(&resolver.stderr)
        );
    }
    let version = String::from_utf8(resolver.stdout)
        .expect("product version is UTF-8")
        .trim()
        .to_string();
    if version.is_empty() {
        panic!("product version resolver returned an empty version");
    }
    let build_number = env::var("TELEVYBACKUP_BUILD_NUMBER").unwrap_or_else(|_| "0".to_string());
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_VERSION={version}");
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_NUMBER={build_number}");
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_LONG_VERSION={version} ({commit})");
    println!("cargo:rerun-if-changed={}", root.join("VERSION").display());
    println!("cargo:rerun-if-changed={}", root.join("scripts/product-version.py").display());
    println!("cargo:rerun-if-env-changed=TELEVYBACKUP_BUILD_MODE");
    println!("cargo:rerun-if-env-changed=TELEVYBACKUP_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=TELEVYBACKUP_BUILD_NUMBER");
}
