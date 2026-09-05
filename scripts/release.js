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
  ROOT, BUILD_TMP, BUILD_OUT,
  BACKEND_MANIFEST, BACKEND_TARGET,
  step, header, run, collectBinaries, listDir, buildModules, installFrontend,
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
if (fs.existsSync(BUILD_OUT)) fs.rmSync(BUILD_OUT, { recursive: true, force: true });

// 支持交叉编译：若环境变量 CARGO_BUILD_TARGET 存在，则加 --target 参数
// Cross-compilation support: if CARGO_BUILD_TARGET is set, pass --target to cargo
const cargoTarget = process.env.CARGO_BUILD_TARGET || null;
const targetFlag  = cargoTarget ? ` --target ${cargoTarget}` : "";

// 平台标识符：优先读环境变量，未设置时按当前宿主机推导
// Platform identifier: prefer env var, fall back to detecting the current host
function detectPlatform() {
  const archStr = process.arch === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "win32")  return `windows-${archStr}`;
  if (process.platform === "darwin") return `darwin-${archStr}`;
  return `linux-${archStr}`;
}
const platform = process.env.CARGO_BUILD_PLATFORM || detectPlatform();
// 交叉编译时产物在 <target_dir>/<triple>/release/，原生编译时在 <target_dir>/release/
// Cross-compiled artifacts live at <target_dir>/<triple>/release/; native at <target_dir>/release/
const backendReleaseDir = cargoTarget
  ? path.join(BACKEND_TARGET, cargoTarget, "release")
  : path.join(BACKEND_TARGET, "release");

run(`cargo build --manifest-path "${BACKEND_MANIFEST}" --release${targetFlag}`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});

// ── Step 4: 模块 / Modules ───────────────────────────────────────────────────
step(4, TOTAL, "Building modules (release) → build/modules/");
const BUILD_MODULES_OUT = path.join(BUILD_OUT, "modules");
buildModules("release", BUILD_MODULES_OUT, cargoTarget, platform);

// ── Step 5: 收集后端主程序 / Collect backend binary ──────────────────────────
step(5, TOTAL, "Collecting backend binary → build/");
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
