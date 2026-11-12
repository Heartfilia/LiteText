fn main() {
    println!("cargo:rerun-if-env-changed=LITETEXT_VERSION");
    let version = std::env::var("LITETEXT_VERSION").unwrap_or_else(|_| "dev".into());
    assert!(!version.contains(['\n', '\r']), "Invalid release version");
    if version != "dev" {
        semver::Version::parse(version.strip_prefix('v').unwrap_or(&version))
            .expect("LITETEXT_VERSION must be a semver tag, for example v1.2.3");
    }
    println!("cargo:rustc-env=LITETEXT_VERSION={version}");
}
