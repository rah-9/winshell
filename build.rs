//! Embeds the icon, application manifest (asInvoker, PerMonitorV2 DPI, UTF-8,
//! Windows 10/11) and version information into winshell.exe (PLAN.md section 10).
//! Uses rc.exe (MSVC) or windres (GNU) via the embed-resource crate.

use std::path::PathBuf;

fn main() {
    for f in ["resources/winshell.rc.in", "resources/winshell.manifest", "resources/winshell.ico"] {
        println!("cargo:rerun-if-changed={f}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // `cargo check --target x86_64-pc-windows-msvc` from Linux has no rc.exe; resources
    // only matter for a linked exe, which needs a Windows MSVC toolchain anyway.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") && !cfg!(windows) {
        println!("cargo:warning=skipping Windows resources (MSVC target built from a non-Windows host)");
        return;
    }
    let v = |k: &str| std::env::var(k).unwrap_or_else(|_| "0".into());
    let (major, minor, patch) =
        (v("CARGO_PKG_VERSION_MAJOR"), v("CARGO_PKG_VERSION_MINOR"), v("CARGO_PKG_VERSION_PATCH"));
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let res_dir = manifest_dir.join("resources").to_string_lossy().replace('\\', "/");
    let template =
        std::fs::read_to_string(manifest_dir.join("resources/winshell.rc.in")).expect("resources/winshell.rc.in");
    let rc = template
        .replace("@VERSION_NUM@", &format!("{major},{minor},{patch},0"))
        .replace("@VERSION_STR@", &format!("{major}.{minor}.{patch}"))
        .replace("@DIR@", &res_dir);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("winshell.rc");
    std::fs::write(&out, rc).expect("write winshell.rc");
    embed_resource::compile_for(&out, ["winshell"], embed_resource::NONE).manifest_required().unwrap();
}
