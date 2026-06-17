#!/usr/bin/env node
/**
 * Desktop 类型检查 / Desktop type check
 *
 * 1. 安装 desktop 依赖
 * 2. vue-tsc --noEmit
 *
 * Usage: npm run desktop:check
 */

"use strict";

const path = require("path");
const { execSync } = require("child_process");

const ROOT    = path.resolve(__dirname, "..");
const DESKTOP = path.join(ROOT, "desktop");

const C = {
  reset:  "\x1b[0m",
  cyan:   "\x1b[36m",
  yellow: "\x1b[33m",
  gray:   "\x1b[90m",
  bold:   "\x1b[1m",
};

const NESTED = process.env.CHECK_NESTED === "1";

function header(title, desc) {
  if (NESTED) {
    const indent = "    ";
    console.log(`\n${indent}${C.gray}${"┄".repeat(52)}${C.reset}`);
    console.log(`${indent}${C.gray}  ${title}${C.reset}`);
    if (desc) console.log(`${indent}${C.gray}  ${desc}${C.reset}`);
    console.log(`${indent}${C.gray}${"┄".repeat(52)}${C.reset}`);
  } else {
    console.log(`\n${C.cyan}${"═".repeat(60)}${C.reset}`);
    console.log(`${C.bold}  ${title}${C.reset}`);
    if (desc) console.log(`  ${desc}`);
    console.log(`${C.cyan}${"═".repeat(60)}${C.reset}`);
  }
}

function step(current, total, msg) {
  if (NESTED) {
    const indent = "    ";
    console.log(`\n${indent}${C.gray}[${current}/${total}]  ${msg}${C.reset}`);
  } else {
    console.log(`\n${C.yellow}${"─".repeat(60)}${C.reset}`);
    console.log(`  ${C.bold}[${current}/${total}]${C.reset}  ${msg}`);
    console.log(`${C.yellow}${"─".repeat(60)}${C.reset}`);
  }
}

function run(cmd, opts = {}) {
  execSync(cmd, { stdio: "inherit", ...opts });
}

const TOTAL = 2;
header("Desktop Check", "install · vue-tsc");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing desktop dependencies");
run("npm install", { cwd: DESKTOP });

// ── Step 2: 类型检查 / Type check ────────────────────────────────────────────
step(2, TOTAL, "Checking desktop types (vue-tsc --noEmit)");
run("npx vue-tsc --noEmit", { cwd: DESKTOP });

// ── 完成 / Done ──────────────────────────────────────────────────────────────
const indent = NESTED ? "    " : "";
console.log(`\n${indent}Desktop check passed.`);
