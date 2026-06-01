#!/usr/bin/env node
/**
 * 开发模式 / Dev mode
 *
 * 1. 检查前端、后端和所有模块
 * 2. 安装前端依赖并构建（输出到 build_tmp/frontend/dist/）
 * 3. cargo run 启动后端（前端产物通过 RustEmbed 嵌入二进制）
 *
 * 前端文件变化后重新运行此命令即可。
 * Re-run this command after frontend changes.
 *
 * Usage: npm run dev
 */

"use strict";

const {
  ROOT, FRONTEND, BACKEND_MANIFEST, BACKEND_TARGET,
  step, header, run, installFrontend,
} = require("./common");

const TOTAL = 3;
header("Dev", "check → build frontend → run backend (debug)");

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

// ── Step 3: 启动后端 / Start backend ─────────────────────────────────────────
step(3, TOTAL, "Starting backend (debug)");
run(`cargo run --manifest-path "${BACKEND_MANIFEST}"`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});
