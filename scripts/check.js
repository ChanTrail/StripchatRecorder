#!/usr/bin/env node
/**
 * 检查所有目标的类型/编译错误 / Check all targets for type and compile errors
 *
 * 1. 安装前端依赖
 * 2. 前端 vue-tsc 类型检查
 * 3. 后端 cargo check
 * 4. 所有模块 cargo check
 *
 * Usage: npm run check
 */

"use strict";

const {
  FRONTEND, NESTED,
  BACKEND_MANIFEST, BACKEND_TARGET,
  step, header, run, checkModules, installFrontend,
} = require("./common");

const TOTAL = 4;
header("Check", "frontend types · backend · modules");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing frontend dependencies");
installFrontend();

// ── Step 2: 前端 / Frontend ──────────────────────────────────────────────────
step(2, TOTAL, "Checking frontend (vue-tsc)");
run("npx vue-tsc --noEmit", { cwd: FRONTEND });

// ── Step 3: 后端 / Backend ───────────────────────────────────────────────────
step(3, TOTAL, "Checking backend");
run(`cargo check --manifest-path "${BACKEND_MANIFEST}"`, {
  env: { ...process.env, CARGO_TARGET_DIR: BACKEND_TARGET },
});

// ── Step 4: 模块 / Modules ───────────────────────────────────────────────────
step(4, TOTAL, "Checking modules");
checkModules();

// ── 完成 / Done ──────────────────────────────────────────────────────────────
const indent = NESTED ? "    " : "";
console.log(`\n${indent}All checks passed.`);
