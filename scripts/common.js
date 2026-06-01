/**
 * 构建脚本公共工具 / Shared build script utilities
 *
 * 供 dev.js / check.js / release.js 引用，避免重复代码。
 * Used by dev.js, check.js, and release.js to avoid duplication.
 */

"use strict";

const { execSync } = require("child_process");
const path = require("path");
const fs   = require("fs");

// ── 路径常量 / Path constants ────────────────────────────────────────────────

const ROOT        = path.resolve(__dirname, "..");
const FRONTEND    = path.join(ROOT, "frontend");
const MODULES_DIR = path.join(ROOT, "modules");
const BUILD_TMP   = path.join(ROOT, "build_tmp");
const BUILD_OUT   = path.join(ROOT, "build");

/** 后端 Cargo.toml 路径 / Backend Cargo.toml path */
const BACKEND_MANIFEST = path.join(ROOT, "backend", "Cargo.toml");

/** 后端编译产物目录 / Backend target directory */
const BACKEND_TARGET = path.join(BUILD_TMP, "backend", "target");

/** 指定模块的编译产物目录 / Target directory for a given module */
function moduleTarget(name) {
  return path.join(BUILD_TMP, "modules", name, "target");
}

/** 枚举所有可构建的模块名（跳过纯库 crate）/ List all buildable module names (skip pure library crates) */
function listModules() {
  const skip = new Set(["pp_utils"]);
  return fs
    .readdirSync(MODULES_DIR)
    .filter(
      (n) => !skip.has(n) && fs.existsSync(path.join(MODULES_DIR, n, "Cargo.toml"))
    );
}

// ── ANSI 颜色 / ANSI colors ──────────────────────────────────────────────────

const C = {
  reset:  "\x1b[0m",
  cyan:   "\x1b[36m",   // header 边框
  yellow: "\x1b[33m",   // step 边框
  gray:   "\x1b[90m",   // 嵌套时的暗色
  bold:   "\x1b[1m",
};

/** 是否作为子进程被嵌套调用（由 dev/release 通过环境变量注入）
 *  Whether running as a nested subprocess (injected by dev/release via env var) */
const NESTED = process.env.CHECK_NESTED === "1";

// ── 输出工具 / Output helpers ────────────────────────────────────────────────

/** 打印脚本开头的总描述标题 / Print the overall script header
 * @param {string} title  标题 / title
 * @param {string} desc   描述 / description
 */
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

/** 打印分隔线步骤标题 / Print a separator with a step title
 * @param {number} current  当前步骤编号 / current step number
 * @param {number} total    总步骤数 / total steps
 * @param {string} msg      步骤描述 / step description
 */
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

/**
 * 同步执行命令，继承 stdio / Run a command synchronously, inheriting stdio.
 * @param {string} cmd
 * @param {import("child_process").ExecSyncOptions} [opts]
 */
function run(cmd, opts = {}) {
  execSync(cmd, { stdio: "inherit", ...opts });
}

// ── 二进制收集 / Binary collection ──────────────────────────────────────────

/**
 * 收集目录顶层的所有可执行文件。
 * Collect all executable files at the top level of a directory.
 * Windows: *.exe；Linux/macOS: 有执行权限且无扩展名的文件。
 * @param {string} releaseDir
 * @returns {string[]}
 */
function collectBinaries(releaseDir) {
  if (!fs.existsSync(releaseDir)) return [];
  const isWindows = process.platform === "win32";
  return fs.readdirSync(releaseDir).filter((name) => {
    const full = path.join(releaseDir, name);
    const stat = fs.statSync(full);
    if (!stat.isFile()) return false;
    if (isWindows) return name.endsWith(".exe");
    return !path.extname(name) && (stat.mode & 0o111) !== 0;
  });
}

// ── 目录列表 / Directory listing ────────────────────────────────────────────

/**
 * 递归打印目录内容（构建完成后展示产物）。
 * Recursively print directory contents (display artifacts after build).
 * @param {string} dir
 * @param {string} [prefix]
 */
function listDir(dir, prefix = "") {
  for (const name of fs.readdirSync(dir).sort()) {
    const full = path.join(dir, name);
    if (fs.statSync(full).isDirectory()) {
      console.log(`  ${prefix}${name}/`);
      listDir(full, prefix + "  ");
    } else {
      const size = (fs.statSync(full).size / 1024).toFixed(0);
      console.log(`  ${prefix}${name}  (${size} KB)`);
    }
  }
}

// ── 前端依赖安装 / Frontend dependency install ──────────────────────────────

/**
 * 安装前端 npm 依赖（每次执行脚本前调用）。
 * Install frontend npm dependencies (called before each script runs).
 */
function installFrontend() {
  console.log("Installing frontend dependencies...");
  run("npm install", { cwd: FRONTEND });
}

// ── 导出 / Exports ───────────────────────────────────────────────────────────

module.exports = {
  ROOT,
  FRONTEND,
  MODULES_DIR,
  BUILD_TMP,
  BUILD_OUT,
  BACKEND_MANIFEST,
  BACKEND_TARGET,
  NESTED,
  moduleTarget,
  listModules,
  step,
  header,
  run,
  collectBinaries,
  listDir,
  installFrontend,
};
