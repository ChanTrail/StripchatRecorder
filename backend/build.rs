use std::fs;

fn main() {
    // 确保 build_tmp/frontend/dist/ 目录存在，避免 RustEmbed 在目录不存在时编译报错
    // Ensure build_tmp/frontend/dist/ exists so RustEmbed doesn't fail when the frontend hasn't been built yet
    let _ = fs::create_dir_all("../build_tmp/frontend/dist");
}
