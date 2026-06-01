use std::fs;

fn main() {
    // 从 frontend/package.json 读取版本号，与 Cargo.toml 中的版本对比，不一致时编译报错。
    // Read version from frontend/package.json and compare with Cargo.toml; fail if they differ.
    let pkg_json = fs::read_to_string("../frontend/package.json")
        .expect("build.rs: failed to read ../frontend/package.json");

    let version_line = pkg_json
        .lines()
        .find(|l| l.trim().starts_with("\"version\""))
        .expect("build.rs: \"version\" field not found in frontend/package.json");

    let npm_version = version_line
        .split('"')
        .nth(3)
        .expect("build.rs: failed to parse version from frontend/package.json");

    let cargo_version = env!("CARGO_PKG_VERSION");

    assert_eq!(
        npm_version, cargo_version,
        "\n\n版本号不一致 / Version mismatch:\n  frontend/package.json : {npm_version}\n  Cargo.toml            : {cargo_version}\n\n请将两处版本号改为一致后重新构建。\nPlease update both files to the same version before building.\n"
    );

    println!("cargo:rerun-if-changed=../frontend/package.json");

    // 确保 build_tmp/frontend/dist/ 目录存在，避免 RustEmbed 在目录不存在时编译报错
    // Ensure build_tmp/frontend/dist/ exists so RustEmbed doesn't fail when the frontend hasn't been built yet
    let _ = fs::create_dir_all("../build_tmp/frontend/dist");
}
