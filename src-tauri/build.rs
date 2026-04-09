use std::fs;

fn main() {
    // 从 package.json 读取版本号，与 Cargo.toml 中的版本对比，不一致时编译报错。
    // Read version from package.json and compare with Cargo.toml; fail the build if they differ.
    let pkg_json = fs::read_to_string("../package.json")
        .expect("build.rs: failed to read ../package.json");

    let version_line = pkg_json
        .lines()
        .find(|l| l.trim().starts_with("\"version\""))
        .expect("build.rs: \"version\" field not found in package.json");

    // 提取引号内的版本字符串，例如 `"version": "1.2.3"` → `1.2.3`
    // Extract the version string from quotes, e.g. `"version": "1.2.3"` → `1.2.3`
    let npm_version = version_line
        .split('"')
        .nth(3)
        .expect("build.rs: failed to parse version from package.json");

    let cargo_version = env!("CARGO_PKG_VERSION");

    assert_eq!(
        npm_version, cargo_version,
        "\n\n版本号不一致 / Version mismatch:\n  package.json : {npm_version}\n  Cargo.toml   : {cargo_version}\n\n请将两处版本号改为一致后重新构建。\nPlease update both files to the same version before building.\n"
    );

    // 将 package.json 加入 rerun 依赖，版本变更时自动重新执行 build.rs
    // Re-run build.rs whenever package.json changes
    println!("cargo:rerun-if-changed=../package.json");

    // 确保 dist/ 目录存在，避免 RustEmbed 在目录不存在时编译报错
    // Ensure dist/ exists so RustEmbed doesn't fail when the frontend hasn't been built yet
    let _ = fs::create_dir_all("../dist");

    tauri_build::build()
}
