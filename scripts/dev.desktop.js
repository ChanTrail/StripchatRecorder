#!/usr/bin/env node
/**
 * Desktop 开发模式 / Desktop dev mode
 *
 * 1. 安装 desktop 依赖
 * 2. 构建所有模块（debug），复制二进制到 Tauri app_data_dir()/modules/
 *    （与打包后行为一致，见 desktop-tauri/src/lib.rs 的 exe_dir 覆盖机制）
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
  step, header, run, buildModules, installDesktop, desktopAppDataDir,
} = require("./common");

/**
 * 模块二进制的目标目录（Tauri 运行时从此处加载）。
 * desktop-tauri 在 setup() 阶段用 app_data_dir() 覆盖了默认的 exe-relative
 * 数据目录（见 backend/src/config/app_state.rs::set_exe_dir_override），
 * dev 模式与打包后行为一致，因此这里必须复制到同一个目录，而不是
 * build_tmp/desktop/target/debug/ 下（那是可执行文件所在目录，已不再是
 * 模块/配置/录制文件的查找位置）。
 *
 * Target directory for module binaries (loaded by Tauri at runtime).
 * desktop-tauri overrides the default exe-relative data directory with
 * app_data_dir() during setup() (see set_exe_dir_override in
 * backend/src/config/app_state.rs); dev mode matches packaged behavior, so
 * binaries must be copied here rather than under build_tmp/desktop/target/debug/
 * (which is just the executable's directory, no longer where modules/config/
 * recordings are looked up).
 */
const MODULES_OUT = path.join(desktopAppDataDir(), "modules");

// 平台标识符：按当前宿主机推导（dev 模式始终为原生编译，无需读环境变量）。
// 必须传给 buildModules，否则复制出的文件名只有 {name}-{version} 两段，
// 会被后端 discovery.rs 的 has_valid_module_filename（要求 ≥3 段）过滤掉，
// 导致模块列表为空。
//
// Platform identifier: detected from the current host (dev always compiles natively).
// Must be passed to buildModules — otherwise copied filenames only have the
// {name}-{version} two segments, which get filtered out by the backend's
// has_valid_module_filename (requires ≥3 segments), resulting in an empty module list.
function detectPlatform() {
  const archStr = process.arch === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "win32")  return `windows-${archStr}`;
  if (process.platform === "darwin") return `darwin-${archStr}`;
  return `linux-${archStr}`;
}
const platform = detectPlatform();

const TOTAL = 3;
header("Desktop Dev", "install → modules → tauri dev");

// ── Step 1: 安装依赖 / Install dependencies ──────────────────────────────────
step(1, TOTAL, "Installing desktop dependencies");
installDesktop();

// ── Step 2: 构建模块并复制 / Build modules & copy binaries ───────────────────
step(2, TOTAL, `Building modules (debug) → ${MODULES_OUT}`);
buildModules("debug", MODULES_OUT, null, platform);

// ── Step 3: 启动 Tauri 开发服务器 / Start Tauri dev ──────────────────────────
step(3, TOTAL, "Starting Tauri dev (build_tmp/desktop/target/)");
run("npx tauri dev", {
  cwd: DESKTOP,
  env: { ...process.env, CARGO_TARGET_DIR: DESKTOP_TARGET },
});
