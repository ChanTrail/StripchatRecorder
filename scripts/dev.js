#!/usr/bin/env node
/**
 * 开发模式 / Dev mode
 *
 * 1. 检查前端、后端和所有模块
 * 2. 安装前端依赖并构建（输出到 build_tmp/frontend/dist/）
 * 3. 构建所有模块（debug），复制二进制到 build_tmp/backend/target/debug/modules/
 * 4. cargo run 启动后端（前端产物通过 RustEmbed 嵌入二进制）
 *
 * 前端或模块文件变化后重新运行此命令即可。
 * Re-run this command after frontend or module changes.
 *
 * Usage: npm run dev
 */

"use strict";

const path = require("path");

const {
  ROOT, FRONTEND, BACKEND_MANIFEST, BACKEND_TARGET,
  step, header, run, buildModules, installFrontend,
} = require("./common");

/** 模块二进制的目标目录（后端运行时从此处加载）
 *  Target directory for module binaries (loaded by the backend at runtime) */
const MODULES_OUT = path.join(BACKEND_TARGET, "debug", "modules");

// 平台标识符：按当前宿主机推导（dev 模式始终为原生编译，无需读环境变量）
// Platform identifier: detected from the current host (dev always compiles natively)
function detectPlatform() {
  const archStr = process.arch === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "win32")  return `windows-${archStr}`;
  if (process.platform === "darwin") return `darwin-${archStr}`;
  return `linux-${archStr}`;
}
const platform = detectPlatform();

const TOTAL = 4;
header("Dev", "check → frontend → modules → run backend (debug)");

// ── Step 1: 检查 / Check ─────────────────────────────────────────────────────
step(1, TOTAL, "Running checks");
run("node scripts/check.js", {
  cwd: ROOT,
  env: { ...process.env, CHECK_NESTED: "1" },
});

// ── Step 2: 构建前端 / Build frontend ────────────────────────────────────────
step(2, TOTAL, "Installing & building frontend");
installFrontend();
run("npm run build", { cwd: FRONTEND });

// ── Step 3: 构建模块并复制 / Build modules & copy binaries ───────────────────
step(3, TOTAL, "Building modules (debug) → build_tmp/backend/target/debug/modules/");
buildModules("debug", MODULES_OUT, null, platform);

// ── Step 4: 启动后端 / Start backend ─────────────────────────────────────────
step(4, TOTAL, "Starting backend (debug)");
run(`cargo run --manifest-path "${BACKEND_MANIFEST}"`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});
