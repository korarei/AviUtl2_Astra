use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=astra.exe.manifest");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    if env::var_os("CARGO_CFG_WINDOWS").is_none() || env::var("CARGO_CFG_TARGET_ENV").ok().as_deref() != Some("msvc") {
        return;
    }

    let ver = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is not set");
    let mut nums = ver.split(['.', '-', '+']);
    let ver = format!(
        "{}.{}.{}.0",
        nums.next().expect("invalid package version"),
        nums.next().expect("invalid package version"),
        nums.next().expect("invalid package version")
    );

    let dst = Path::new(&env::var("OUT_DIR").expect("OUT_DIR is not set")).join("astra.exe.manifest");
    fs::write(&dst, include_str!("astra.exe.manifest").replace("${{VERSION}}", &ver))
        .expect("failed to write application manifest");

    println!("cargo:rustc-link-arg-bin=astra=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin=astra=/MANIFESTINPUT:{}", dst.display());
}
