#!/usr/bin/env node
/**
 * Desktop 类型检查 / Desktop type check
 *
 * 1. 安装 desktop 依赖
 * 2. vue-tsc --noEmit
 * 3. 所有模块 cargo check
 *
 * Usage: npm run check:desktop
 */

"use strict";

const {
  DESKTOP, NESTED,
  step, header, run, checkModules, installDesktop,
} = require("./common");

const TOTAL = 3;
header("Desktop Check", "install · vue-tsc · modules");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing desktop dependencies");
installDesktop();

// ── Step 2: 类型检查 / Type check ────────────────────────────────────────────
step(2, TOTAL, "Checking desktop types (vue-tsc --noEmit)");
run("npx vue-tsc --noEmit", { cwd: DESKTOP });

// ── Step 3: 模块 / Modules ───────────────────────────────────────────────────
step(3, TOTAL, "Checking modules");
checkModules();

// ── 完成 / Done ──────────────────────────────────────────────────────────────
const indent = NESTED ? "    " : "";
console.log(`\n${indent}Desktop check passed.`);
