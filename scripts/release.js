#!/usr/bin/env node
/**
 * Release 构建流程 / Release build pipeline
 *
 * 1. 检查前端、后端和所有模块
 * 2. 安装前端依赖并构建  → build_tmp/frontend/dist/
 * 3. 构建后端 (release)  → build_tmp/backend/target/release/
 * 4. 构建所有模块        → build_tmp/modules/<name>/target/release/
 * 5. 收集可执行文件      → build/
 *    build/
 *    ├── stripchat-recorder
 *    └── modules/
 *        ├── contact_sheet_*
 *        ├── filter_short_*
 *        ├── notify_discord_*
 *        └── notify_telegram_*
 * 6. 删除 build_tmp/
 *
 * Usage: npm run build
 */

"use strict";

const path = require("path");
const fs   = require("fs");

const {
  ROOT, BUILD_TMP, BUILD_OUT, MODULES_DIR,
  BACKEND_MANIFEST, BACKEND_TARGET, moduleTarget,
  listModules, step, header, run, collectBinaries, listDir, installFrontend,
} = require("./common");

const TOTAL = 6;
header("Build", "check → frontend → backend → modules → collect → cleanup");

// ── Step 1: 检查 / Check ─────────────────────────────────────────────────────
step(1, TOTAL, "Running checks");
run("node scripts/check.js", {
  cwd: ROOT,
  env: { ...process.env, CHECK_NESTED: "1" },
});

// ── Step 2: 前端 / Frontend ──────────────────────────────────────────────────
step(2, TOTAL, "Installing & building frontend");
installFrontend();
run("npm run build --prefix frontend", { cwd: ROOT });

// ── Step 3: 后端 / Backend ───────────────────────────────────────────────────
step(3, TOTAL, "Building backend (release)");
run(`cargo build --manifest-path "${BACKEND_MANIFEST}" --release`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});

// ── Step 4: 模块 / Modules ───────────────────────────────────────────────────
step(4, TOTAL, "Building modules (release)");
for (const name of listModules()) {
  console.log(`  → ${name}`);
  run(
    `cargo build --manifest-path "${path.join(MODULES_DIR, name, "Cargo.toml")}" --bins --release`,
    { env: { ...process.env, CARGO_TARGET_DIR: moduleTarget(name) } }
  );
  console.log(`  ✓ ${name}\n`);
}

// ── Step 5: 收集产物 / Collect artifacts ────────────────────────────────────
step(5, TOTAL, "Collecting artifacts → build/");

if (fs.existsSync(BUILD_OUT)) fs.rmSync(BUILD_OUT, { recursive: true, force: true });
const BUILD_MODULES_OUT = path.join(BUILD_OUT, "modules");
fs.mkdirSync(BUILD_MODULES_OUT, { recursive: true });

// 收集后端主程序 / Collect backend binary
const backendReleaseDir = path.join(BACKEND_TARGET, "release");
const backendBins = collectBinaries(backendReleaseDir);
if (backendBins.length === 0) {
  console.error(`ERROR: No backend binary found in ${backendReleaseDir}`);
  process.exit(1);
}
for (const name of backendBins) {
  const dst = path.join(BUILD_OUT, name);
  fs.copyFileSync(path.join(backendReleaseDir, name), dst);
  if (process.platform !== "win32") fs.chmodSync(dst, 0o755);
  console.log(`  ✓ build/${name}`);
}

// 收集模块二进制 / Collect module binaries
for (const name of listModules()) {
  const releaseDir = path.join(moduleTarget(name), "release");
  const bins = collectBinaries(releaseDir);
  if (bins.length === 0) {
    console.warn(`  ⚠ No binaries found for module: ${name}`);
    continue;
  }
  for (const bin of bins) {
    const dst = path.join(BUILD_MODULES_OUT, bin);
    fs.copyFileSync(path.join(releaseDir, bin), dst);
    if (process.platform !== "win32") fs.chmodSync(dst, 0o755);
    console.log(`  ✓ build/modules/${bin}`);
  }
}

// ── Step 6: 清理 / Cleanup ───────────────────────────────────────────────────
step(6, TOTAL, "Cleanup");
fs.rmSync(BUILD_TMP, { recursive: true, force: true });
console.log("  ✓ build_tmp/ removed");

// ── 完成 / Done ──────────────────────────────────────────────────────────────
console.log(`\n${"═".repeat(60)}`);
console.log("  Release build complete!");
console.log(`  Output: build/`);
console.log("═".repeat(60));
listDir(BUILD_OUT);
