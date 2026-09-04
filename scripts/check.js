#!/usr/bin/env node
/**
 * 检查所有目标的类型/编译错误 / Check all targets for type and compile errors
 *
 * 1. 安装前端依赖
 * 2. 前端 vue-tsc 类型检查
 * 3. 后端 cargo check
 * 4. 后端 cargo clippy
 * 5. 所有模块 cargo check
 * 6. 所有模块 cargo clippy
 *
 * Usage: npm run check
 */

"use strict";

const {
  FRONTEND, NESTED,
  BACKEND_MANIFEST, BACKEND_TARGET,
  step, header, run, checkModules, clippyModules, installFrontend,
} = require("./common");

const TOTAL = 6;
header("Check", "frontend types · backend · backend clippy · modules · modules clippy");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing frontend dependencies");
installFrontend();

// ── Step 2: 前端 / Frontend ──────────────────────────────────────────────────
step(2, TOTAL, "Checking frontend (vue-tsc)");
run("npx vue-tsc --noEmit", { cwd: FRONTEND });

// ── Step 3: 后端编译检查 / Backend compile check ─────────────────────────────
step(3, TOTAL, "Checking backend (cargo check)");
run(`cargo check --manifest-path "${BACKEND_MANIFEST}"`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});

// ── Step 4: 后端 Clippy / Backend clippy ─────────────────────────────────────
step(4, TOTAL, "Checking backend (cargo clippy)");
run(`cargo clippy --manifest-path "${BACKEND_MANIFEST}" -- -D warnings`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});

// ── Step 5: 模块编译检查 / Modules compile check ────────────────────────────
step(5, TOTAL, "Checking modules (cargo check)");
checkModules();

// ── Step 6: 模块 Clippy / Modules clippy ─────────────────────────────────────
step(6, TOTAL, "Checking modules (cargo clippy)");
clippyModules();

// ── 完成 / Done ──────────────────────────────────────────────────────────────
const indent = NESTED ? "    " : "";
console.log(`\n${indent}All checks passed.`);
