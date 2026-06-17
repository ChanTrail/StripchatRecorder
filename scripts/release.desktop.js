#!/usr/bin/env node
/**
 * Desktop 构建 / Desktop build
 *
 * 1. 类型检查（vue-tsc --noEmit）
 * 2. 安装 desktop 依赖
 * 3. Vite 构建（vite build）→ desktop/dist/
 *
 * Usage: npm run desktop:build
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
  bold:   "\x1b[1m",
};

function header(title, desc) {
  console.log(`\n${C.cyan}${"═".repeat(60)}${C.reset}`);
  console.log(`${C.bold}  ${title}${C.reset}`);
  if (desc) console.log(`  ${desc}`);
  console.log(`${C.cyan}${"═".repeat(60)}${C.reset}`);
}

function step(current, total, msg) {
  console.log(`\n${C.yellow}${"─".repeat(60)}${C.reset}`);
  console.log(`  ${C.bold}[${current}/${total}]${C.reset}  ${msg}`);
  console.log(`${C.yellow}${"─".repeat(60)}${C.reset}`);
}

function run(cmd, opts = {}) {
  execSync(cmd, { stdio: "inherit", ...opts });
}

const TOTAL = 3;
header("Desktop Build", "check → install → vite build");

// ── Step 1: 类型检查 / Type check ────────────────────────────────────────────
step(1, TOTAL, "Type checking desktop (vue-tsc)");
run("node scripts/desktop_check.js", {
  cwd: ROOT,
  env: { ...process.env, CHECK_NESTED: "1" },
});

// ── Step 2: 安装依赖 / Install dependencies ──────────────────────────────────
step(2, TOTAL, "Installing desktop dependencies");
run("npm install", { cwd: DESKTOP });

// ── Step 3: Vite 构建 / Vite build ───────────────────────────────────────────
step(3, TOTAL, "Building desktop (vite build) → desktop/dist/");
run("npx vite build", { cwd: DESKTOP });

// ── 完成 / Done ──────────────────────────────────────────────────────────────
console.log(`\n${"═".repeat(60)}`);
console.log("  Desktop build complete!");
console.log("  Output: desktop/dist/");
console.log("═".repeat(60));
