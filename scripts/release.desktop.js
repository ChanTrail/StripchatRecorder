#!/usr/bin/env node
/**
 * Desktop Release 构建 / Desktop release build
 *
 * 1. 类型检查（vue-tsc --noEmit）+ 所有模块 cargo check
 * 2. 安装 desktop 依赖
 * 3. 构建所有模块（release），复制二进制到 build_tmp/desktop/target/release/modules/
 * 4. Tauri 构建（tauri build）
 *    - 前端由 Tauri 的 beforeBuildCommand 自动执行 vite build → desktop/dist/
 *    - Rust 编译产物重定向到 build_tmp/desktop/target/
 * 5. 收集 bundle 产物 → build/desktop/
 * 6. 收集模块二进制 → build/desktop/modules/
 * 7. 清理 build_tmp/desktop/
 *
 * Usage: npm run build:desktop
 */

"use strict";

const path = require("path");
const fs   = require("fs");

const {
  ROOT, DESKTOP, DESKTOP_TARGET, BUILD_OUT, BUILD_TMP, NESTED,
  step, header, run, listDir, collectBinaries, buildModules, copyDir, installDesktop,
  readDesktopIdentifier,
} = require("./common");

// 平台标识符：优先读环境变量（跨平台 CI 交叉打包时使用），否则按当前宿主机推导。
// 必须传给 buildModules，否则复制出的文件名只有 {name}-{version} 两段，
// 会被后端 discovery.rs 的 has_valid_module_filename（要求 ≥3 段）过滤掉，
// 导致模块列表为空（此前 desktop release 构建一直存在这个问题）。
//
// Platform identifier: prefer env var (used for cross-platform CI packaging),
// else detect from the current host. Must be passed to buildModules — otherwise
// copied filenames only have the {name}-{version} two segments, which get filtered
// out by the backend's has_valid_module_filename (requires ≥3 segments), resulting
// in an empty module list (this bug has been present in desktop release builds).
function detectPlatform() {
  const archStr = process.arch === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "win32")  return `windows-${archStr}`;
  if (process.platform === "darwin") return `darwin-${archStr}`;
  return `linux-${archStr}`;
}
const platform = process.env.CARGO_BUILD_PLATFORM || detectPlatform();

/** Tauri bundle 产物源目录 / Tauri bundle output source */
const TAURI_BUNDLE_SRC = path.join(DESKTOP_TARGET, "release", "bundle");

/** Desktop 产物收集目标目录 / Desktop artifact collection target */
const DESKTOP_BUILD_OUT = path.join(BUILD_OUT, "desktop");

/** desktop build_tmp 目录 / Desktop build_tmp directory */
const DESKTOP_BUILD_TMP = path.join(BUILD_TMP, "desktop");

const TOTAL = 7;
header("Desktop Build", "check → install → modules → tauri build → collect → collect modules → cleanup");

// ── Step 1: 类型检查 / Type check ────────────────────────────────────────────
step(1, TOTAL, "Type checking desktop + modules");
run("node scripts/check.desktop.js", {
  cwd: ROOT,
  env: { ...process.env, CHECK_NESTED: "1" },
});

// ── Step 2: 安装依赖 / Install dependencies ──────────────────────────────────
step(2, TOTAL, "Installing desktop dependencies");
if (fs.existsSync(DESKTOP_BUILD_OUT)) fs.rmSync(DESKTOP_BUILD_OUT, { recursive: true, force: true });
installDesktop();

// ── Step 3: 构建模块并复制 / Build modules & copy binaries ───────────────────
step(3, TOTAL, "Building modules (release) → build/desktop/modules/");
const DESKTOP_MODULES_OUT = path.join(DESKTOP_BUILD_OUT, "modules");
buildModules("release", DESKTOP_MODULES_OUT, null, platform);

// ── Step 4: Tauri 构建 / Tauri build ─────────────────────────────────────────
step(4, TOTAL, "Building desktop (tauri build) → build_tmp/desktop/target/");
run("npx tauri build", {
  cwd: DESKTOP,
  env: { ...process.env, CARGO_TARGET_DIR: DESKTOP_TARGET },
});

// ── Step 5: 收集 bundle 产物 / Collect bundle artifacts ──────────────────────
step(5, TOTAL, "Collecting bundle artifacts → build/desktop/");

if (!fs.existsSync(TAURI_BUNDLE_SRC)) {
  console.error(`ERROR: Tauri bundle directory not found: ${TAURI_BUNDLE_SRC}`);
  process.exit(1);
}
copyDir(TAURI_BUNDLE_SRC, DESKTOP_BUILD_OUT, "build/desktop/");

// ── Step 6: 收集模块二进制 / Collect module binaries ─────────────────────────
step(6, TOTAL, "Collecting module binaries → build/desktop/modules/");
const moduleBins = collectBinaries(DESKTOP_MODULES_OUT);
if (moduleBins.length === 0) {
  console.warn("  ⚠ No module binaries found");
} else {
  for (const bin of moduleBins) {
    console.log(`  ✓ build/desktop/modules/${bin}`);
  }
}

// 用户安装打包产物后，模块二进制需要手动放到 Tauri app_data_dir()（每用户标准数据
// 目录，见 backend/src/config/app_state.rs::set_exe_dir_override）下的 modules/ 子
// 目录 —— 不再是可执行文件所在目录（安装包场景下该目录常只读/无权限/属于签名产物，
// 详见该文档注释）。三平台的具体路径见 README。
//
// After installing the packaged app, module binaries must be placed manually under
// the modules/ subdirectory of Tauri's app_data_dir() (the OS-standard per-user data
// directory, see set_exe_dir_override in backend/src/config/app_state.rs) — no longer
// the executable's directory (which, for installer packages, is often read-only,
// unwritable, or part of a signed artifact; see that doc comment for details). See
// README for the exact per-platform path.
const desktopIdentifier = readDesktopIdentifier();
console.log("\n  ⚠ Module install path (per-user data directory, not next to the executable):");
console.log(`      Windows: %APPDATA%\\${desktopIdentifier}\\modules\\`);
console.log(`      macOS:   ~/Library/Application Support/${desktopIdentifier}/modules/`);
console.log(`      Linux:   ~/.local/share/${desktopIdentifier}/modules/  (or $XDG_DATA_HOME)`);

// ── Step 7: 清理 / Cleanup ───────────────────────────────────────────────────
step(7, TOTAL, "Cleanup");
fs.rmSync(DESKTOP_BUILD_TMP, { recursive: true, force: true });
console.log("  ✓ build_tmp/desktop/ removed");

// ── 完成 / Done ──────────────────────────────────────────────────────────────
console.log(`\n${"═".repeat(60)}`);
console.log("  Desktop build complete!");
console.log("  Output: build/desktop/");
console.log("═".repeat(60));
listDir(DESKTOP_BUILD_OUT);
