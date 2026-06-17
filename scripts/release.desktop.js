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
} = require("./common");

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
buildModules("release", DESKTOP_MODULES_OUT);

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
