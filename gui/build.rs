fn main() {
    // Windows: embed a manifest (UAC: run as administrator, DPI aware, long paths) + version info.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows/app.rc");
        println!("cargo:rerun-if-changed=windows/app.manifest");
        let _ = embed_resource::compile("windows/app.rc", embed_resource::NONE);
    }
}
