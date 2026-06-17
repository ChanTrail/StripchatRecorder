#!/usr/bin/env node
/**
 * Desktop 开发模式 / Desktop dev mode
 *
 * 1. 安装 desktop 依赖
 * 2. 构建所有模块（debug），复制二进制到 build_tmp/desktop/target/debug/modules/
 * 3. 启动 Tauri 开发服务器（tauri dev）
 *    - 前端 Vite 开发服务器由 Tauri 的 beforeDevCommand 自动启动
 *    - Rust 编译产物重定向到 build_tmp/desktop/target/
 *
 * Usage: npm run dev:desktop
 */

"use strict";

const path = require("path");

const {
  DESKTOP, DESKTOP_TARGET,
  step, header, run, buildModules, installDesktop,
} = require("./common");

/** 模块二进制的目标目录（Tauri 运行时从此处加载）
 *  Target directory for module binaries (loaded by Tauri at runtime) */
const MODULES_OUT = path.join(DESKTOP_TARGET, "debug", "modules");

const TOTAL = 3;
header("Desktop Dev", "install → modules → tauri dev");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing desktop dependencies");
installDesktop();

// ── Step 2: 构建模块并复制 / Build modules & copy binaries ───────────────────
step(2, TOTAL, "Building modules (debug) → build_tmp/desktop/target/debug/modules/");
buildModules("debug", MODULES_OUT);

// ── Step 3: 启动 Tauri 开发服务器 / Start Tauri dev ──────────────────────────
step(3, TOTAL, "Starting Tauri dev (build_tmp/desktop/target/)");
run("npx tauri dev", {
  cwd: DESKTOP,
  env: { ...process.env, CARGO_TARGET_DIR: DESKTOP_TARGET },
});
