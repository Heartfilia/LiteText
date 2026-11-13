fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-env-changed=LITETEXT_VERSION");
    let version = std::env::var("LITETEXT_VERSION").unwrap_or_else(|_| "dev".into());
    assert!(!version.contains(['\n', '\r']), "Invalid release version");
    if version != "dev" {
        semver::Version::parse(version.strip_prefix('v').unwrap_or(&version))
            .expect("LITETEXT_VERSION must be a semver tag, for example v1.2.3");
    }
    println!("cargo:rustc-env=LITETEXT_VERSION={version}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app-icon.ico")
            .set("ProductName", "LiteText")
            .set("FileDescription", "LiteText text editor")
            .compile()
            .expect("Unable to embed Windows executable icon");
    }
}
