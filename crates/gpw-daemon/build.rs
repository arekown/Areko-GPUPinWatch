// Embeds the Areko GPUPinWatch icon into the Windows executable (no-op on other targets).
fn main() {
    println!("cargo:rerun-if-changed=gpw-daemon.rc");
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("gpw-daemon.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
