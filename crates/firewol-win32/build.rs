fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    println!("cargo:rerun-if-changed=firewol.manifest");
    let manifest = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("firewol.manifest");
    println!("cargo:rustc-link-arg-bin=firewol-win32=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=firewol-win32=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
