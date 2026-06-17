#!/usr/bin/env node
/**
 * Desktop 开发模式 / Desktop dev mode
 *
 * 1. 安装 desktop 依赖
 * 2. 启动 Vite 开发服务器（vite）
 *
 * Usage: npm run desktop:dev
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

const TOTAL = 2;
header("Desktop Dev", "install → vite dev server");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing desktop dependencies");
run("npm install", { cwd: DESKTOP });

// ── Step 2: 启动开发服务器 / Start dev server ────────────────────────────────
step(2, TOTAL, "Starting Vite dev server");
run("npm run dev", { cwd: DESKTOP });
