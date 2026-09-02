use std::env;

fn main() {
    let version = env::var("TELEVYBACKUP_BUILD_VERSION")
        .unwrap_or_else(|_| env::var("CARGO_PKG_VERSION").expect("cargo package version"));
    let commit = env::var("TELEVYBACKUP_BUILD_COMMIT").unwrap_or_else(|_| "unknown".to_string());
    let build_number = env::var("TELEVYBACKUP_BUILD_NUMBER").unwrap_or_else(|_| "0".to_string());
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_VERSION={version}");
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_NUMBER={build_number}");
    println!("cargo:rustc-env=TELEVYBACKUP_BUILD_LONG_VERSION={version} ({commit})");
    println!("cargo:rerun-if-env-changed=TELEVYBACKUP_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=TELEVYBACKUP_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=TELEVYBACKUP_BUILD_NUMBER");
}
