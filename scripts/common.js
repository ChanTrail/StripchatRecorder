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
const DESKTOP     = path.join(ROOT, "desktop");
const MODULES_DIR = path.join(ROOT, "modules");
const BUILD_TMP   = path.join(ROOT, "build_tmp");
const BUILD_OUT   = path.join(ROOT, "build");

/** 后端 Cargo.toml 路径 / Backend Cargo.toml path */
const BACKEND_MANIFEST = path.join(ROOT, "backend", "Cargo.toml");

/** 后端编译产物目录 / Backend target directory */
const BACKEND_TARGET = path.join(BUILD_TMP, "backend", "target");

/** Desktop (Tauri) 编译产物目录 / Desktop (Tauri) target directory */
const DESKTOP_TARGET = path.join(BUILD_TMP, "desktop", "target");

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

/**
 * 从模块自身 Cargo.toml 的 [package] 段读取 version 字段。
 * 只在 [package] 段内匹配，避免误读 [dependencies] 等其他段中同名字段
 * （如 `grammers-client = { version = "0.10" }`）。
 *
 * Read the `version` field from a module's own Cargo.toml [package] section.
 * Only matches within [package] to avoid misreading a same-named field from
 * other sections (e.g. `grammers-client = { version = "0.10" }` under [dependencies]).
 *
 * @param {string} cargoTomlPath
 * @returns {string|null}
 */
function readPackageVersion(cargoTomlPath) {
  const content = fs.readFileSync(cargoTomlPath, "utf8");
  let inPackage = false;
  for (const rawLine of content.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (/^\[.*\]$/.test(line)) {
      inPackage = line === "[package]";
      continue;
    }
    if (inPackage) {
      const m = line.match(/^version\s*=\s*"([^"]+)"/);
      if (m) return m[1];
    }
  }
  return null;
}

/**
 * 判断 outDir 下的某个文件名是否是指定模块 stem 遗留的旧产物（无论是否带版本号
 * 后缀），需要在复制本次新构建的二进制前清理掉，避免新旧文件并存导致
 * discover_modules 扫描到重复的模块 id。
 *
 * 匹配范围涵盖：裸文件名（`{stem}` / `{stem}.exe`，见于本次刚构建、尚未改名前的
 * 中间产物或更早版本的构建脚本）、新的版本化命名（`{stem}-0.5.0.exe`）、以及历史上
 * 曾经手动在 Cargo.toml 的 `[[bin]] name` 里内嵌版本号的旧约定（`{stem}_v030.exe`）——
 * 后两者的共同特征是 stem 后紧跟 `-` 或 `_`。
 *
 * Determine whether a filename in outDir is a stale leftover artifact for the given
 * module stem (versioned or not), which must be cleaned up before copying this build's
 * new binary — otherwise old and new files would coexist and discover_modules would see
 * a duplicate module id.
 *
 * Covers: bare filenames (`{stem}` / `{stem}.exe`, from this build's intermediate output
 * before renaming, or from an older version of this build script), the new versioned
 * naming (`{stem}-0.5.0.exe`), and the legacy convention of hand-embedding the version
 * into Cargo.toml's `[[bin]] name` (`{stem}_v030.exe`) — the latter two share the trait
 * of stem being immediately followed by `-` or `_`.
 *
 * @param {string} fileName
 * @param {string} stem
 * @returns {boolean}
 */
function isStaleModuleBinary(fileName, stem) {
  if (fileName === stem || fileName === `${stem}.exe`) return true;
  if (!fileName.startsWith(stem)) return false;
  const nextChar = fileName[stem.length];
  return nextChar === "-" || nextChar === "_";
}

/**
 * 清理 outDir 下指定模块 stem 的所有历史遗留产物文件。
 * Remove all stale leftover artifact files for the given module stem in outDir.
 *
 * @param {string} outDir
 * @param {string} stem
 */
function removeStaleModuleBinaries(outDir, stem) {
  if (!fs.existsSync(outDir)) return;
  for (const f of fs.readdirSync(outDir)) {
    if (isStaleModuleBinary(f, stem)) {
      fs.rmSync(path.join(outDir, f), { force: true });
    }
  }
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

// ── 模块构建与检查 / Module build & check ───────────────────────────────────

/**
 * 对所有模块执行 cargo check。
 * Run `cargo check` for all modules.
 */
function checkModules() {
  for (const name of listModules()) {
    run(
      `cargo check --manifest-path "${path.join(MODULES_DIR, name, "Cargo.toml")}"`,
      { env: { ...process.env, CARGO_TARGET_DIR: moduleTarget(name) } }
    );
  }
}

/**
 * 构建所有模块并将产物二进制复制到指定目录，复制时文件名附加模块自身
 * Cargo.toml 中的 version 号（如 `notify_telegram-0.5.0.exe`），方便用户在
 * modules/ 目录中直接从文件名分辨版本，不必逐个运行 --describe。
 *
 * 复制前会清理 outDir 下该模块的所有历史遗留产物（不论是否带版本号后缀、
 * 不论是本次构建脚本的中间产物还是更早版本命名约定留下的文件），确保同一
 * 模块任何时候在 outDir 下只有"本次构建的版本"这一份文件——避免新旧版本
 * 并存导致后端 discover_modules 扫描到重复的模块 id（曾发生过：升级模块
 * 版本后旧文件未清理，新旧文件并存，后端随机选中其中一个，导致版本号显示
 * 不一致、或运行的是修复 bug 之前的旧版本）。
 *
 * Build all modules and copy output binaries to the given directory, appending each
 * module's own Cargo.toml version to the copied filename (e.g.
 * `notify_telegram-0.5.0.exe`), so users can tell versions apart directly from
 * filenames in modules/ without running --describe on each one.
 *
 * Before copying, all of that module's stale leftover artifacts in outDir are removed
 * (versioned or not, whether left by an earlier run of this build script or by an older
 * naming convention) — ensuring outDir always has exactly one file per module: the one
 * from this build. This avoids old and new versions coexisting, which previously caused
 * the backend's discover_modules to see a duplicate module id (this actually happened:
 * after bumping a module's version, the old file wasn't cleaned up, both coexisted, and
 * the backend would nondeterministically pick either one — causing inconsistent version
 * display, or running a stale binary predating a bug fix).
 *
 * @param {"debug"|"release"} profile  Cargo 构建模式 / Cargo build profile
 * @param {string} outDir              二进制复制目标目录 / Target directory for copied binaries
 */
function buildModules(profile, outDir) {
  const releaseFlag = profile === "release" ? " --release" : "";
  fs.mkdirSync(outDir, { recursive: true });
  for (const name of listModules()) {
    console.log(`  → ${name}`);
    const manifestPath = path.join(MODULES_DIR, name, "Cargo.toml");
    run(
      `cargo build --manifest-path "${manifestPath}" --bins${releaseFlag}`,
      { env: { ...process.env, CARGO_TARGET_DIR: moduleTarget(name) } }
    );
    const version = readPackageVersion(manifestPath);
    const bins = collectBinaries(path.join(moduleTarget(name), profile));
    for (const bin of bins) {
      const ext = process.platform === "win32" ? ".exe" : "";
      const stem = ext ? bin.slice(0, -ext.length) : bin;
      removeStaleModuleBinaries(outDir, stem);
      const dstName = version ? `${stem}-${version}${ext}` : bin;
      const dst = path.join(outDir, dstName);
      fs.copyFileSync(path.join(moduleTarget(name), profile, bin), dst);
      if (process.platform !== "win32") fs.chmodSync(dst, 0o755);
    }
    console.log(`  ✓ ${name}\n`);
  }
}

/**
 * 递归复制目录（构建完成后收集产物用）。
 * Recursively copy a directory (used for collecting build artifacts).
 *
 * @param {string} src       源目录 / Source directory
 * @param {string} dst       目标目录 / Destination directory
 * @param {string} [logBase] 用于日志输出的基准路径前缀 / Base path prefix for log output
 */
function copyDir(src, dst, logBase) {
  fs.mkdirSync(dst, { recursive: true });
  for (const entry of fs.readdirSync(src)) {
    const srcPath = path.join(src, entry);
    const dstPath = path.join(dst, entry);
    if (fs.statSync(srcPath).isDirectory()) {
      copyDir(srcPath, dstPath, logBase);
    } else {
      fs.copyFileSync(srcPath, dstPath);
      if (logBase !== undefined) {
        console.log(`  ✓ ${logBase}${path.relative(dst, dstPath).replace(/\\/g, "/")}`);
      }
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

/**
 * 安装 desktop npm 依赖（每次执行 desktop 脚本前调用）。
 * Install desktop npm dependencies (called before each desktop script runs).
 */
function installDesktop() {
  console.log("Installing desktop dependencies...");
  run("npm install", { cwd: DESKTOP });
}

// ── 导出 / Exports ───────────────────────────────────────────────────────────

module.exports = {
  ROOT,
  FRONTEND,
  DESKTOP,
  MODULES_DIR,
  BUILD_TMP,
  BUILD_OUT,
  BACKEND_MANIFEST,
  BACKEND_TARGET,
  DESKTOP_TARGET,
  NESTED,
  moduleTarget,
  listModules,
  readPackageVersion,
  isStaleModuleBinary,
  removeStaleModuleBinaries,
  step,
  header,
  run,
  collectBinaries,
  listDir,
  checkModules,
  buildModules,
  copyDir,
  installFrontend,
  installDesktop,
};
