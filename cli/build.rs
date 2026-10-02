fn main() {
    // Windows: manifest (UTF-8 code page, long paths, runs without elevation) + icon.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows/cli.rc");
        println!("cargo:rerun-if-changed=windows/cli.manifest");
        let _ = embed_resource::compile("windows/cli.rc", embed_resource::NONE);
    }
}
