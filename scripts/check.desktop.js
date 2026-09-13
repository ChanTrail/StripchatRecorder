#!/usr/bin/env node
/**
 * Desktop 类型检查 / Desktop type check
 *
 * 1. 安装 desktop 依赖
 * 2. vue-tsc --noEmit
 * 3. desktop src-tauri cargo check
 * 4. desktop src-tauri cargo clippy
 * 5. 所有模块 cargo check
 * 6. 所有模块 cargo clippy
 *
 * Usage: npm run check:desktop
 */

"use strict";

const path = require("path");
const {
  ROOT, DESKTOP, DESKTOP_TARGET, NESTED,
  step, header, run, checkModules, clippyModules, installDesktop,
} = require("./common");

const DESKTOP_MANIFEST = path.join(ROOT, "desktop", "src-tauri", "Cargo.toml");

const TOTAL = 6;
header("Desktop Check", "install · vue-tsc · cargo check · cargo clippy · modules · modules clippy");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing desktop dependencies");
installDesktop();

// ── Step 2: 前端类型检查 / Frontend type check ───────────────────────────────
step(2, TOTAL, "Checking desktop frontend (vue-tsc --noEmit)");
run("npx vue-tsc --noEmit", { cwd: DESKTOP });

// ── Step 3: Tauri 后端编译检查 / Tauri backend compile check ─────────────────
step(3, TOTAL, "Checking desktop backend (cargo check)");
run(`cargo check --manifest-path "${DESKTOP_MANIFEST}"`, {
  env: { ...process.env, CARGO_TARGET_DIR: DESKTOP_TARGET },
});

// ── Step 4: Tauri 后端 Clippy / Tauri backend clippy ─────────────────────────
step(4, TOTAL, "Checking desktop backend (cargo clippy)");
run(`cargo clippy --manifest-path "${DESKTOP_MANIFEST}" -- -D warnings`, {
  env: { ...process.env, CARGO_TARGET_DIR: DESKTOP_TARGET },
});

// ── Step 5: 模块编译检查 / Modules compile check ────────────────────────────
step(5, TOTAL, "Checking modules (cargo check)");
checkModules();

// ── Step 6: 模块 Clippy / Modules clippy ─────────────────────────────────────
step(6, TOTAL, "Checking modules (cargo clippy)");
clippyModules();

// ── 完成 / Done ──────────────────────────────────────────────────────────────
const indent = NESTED ? "    " : "";
console.log(`\n${indent}Desktop check passed.`);
