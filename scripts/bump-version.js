#!/usr/bin/env node
/**
 * 版本号同步工具 / Version bump utility
 *
 * 一次性将所有需要保持一致的版本号文件更新为指定版本。
 * Updates all version-bearing files to a given version in one pass.
 *
 * 涉及文件 / Files updated:
 *   - package.json                        (根 workspace / root workspace)
 *   - frontend/package.json
 *   - backend/Cargo.toml
 *   - desktop/package.json
 *   - desktop/src-tauri/Cargo.toml
 *   - desktop/src-tauri/tauri.conf.json
 *
 * 用法 / Usage:
 *   node scripts/bump-version.js <new-version>
 *   npm run bump-version -- <new-version>
 *
 * 示例 / Example:
 *   node scripts/bump-version.js 0.4.0
 */

"use strict";

const fs   = require("fs");
const path = require("path");

// ── 工具函数 / Helpers ────────────────────────────────────────────────────────

const C = {
  reset:  "\x1b[0m",
  cyan:   "\x1b[36m",
  green:  "\x1b[32m",
  yellow: "\x1b[33m",
  red:    "\x1b[31m",
  bold:   "\x1b[1m",
  gray:   "\x1b[90m",
};

function ok(msg)   { console.log(`  ${C.green}✓${C.reset}  ${msg}`); }
function warn(msg) { console.log(`  ${C.yellow}⚠${C.reset}  ${msg}`); }
function fail(msg) { console.error(`  ${C.red}✗${C.reset}  ${msg}`); }

// ── 版本校验 / Version validation ────────────────────────────────────────────

const SEM_VER = /^\d+\.\d+\.\d+$/;

function validateVersion(v) {
  if (!SEM_VER.test(v)) {
    fail(`"${v}" 不是合法的语义化版本号（需符合 X.Y.Z 格式）。`);
    fail(`"${v}" is not a valid semver string (must match X.Y.Z).`);
    process.exit(1);
  }
}

// ── 文件修改函数 / File patchers ─────────────────────────────────────────────

/**
 * 更新 JSON 文件的顶层 "version" 字段，保留原始缩进风格。
 * Update the top-level "version" field in a JSON file, preserving its indent style.
 */
function patchJson(filePath, newVersion) {
  const raw = fs.readFileSync(filePath, "utf8");
  const indent = detectJsonIndent(raw);
  const obj = JSON.parse(raw);
  const oldVersion = obj.version;
  obj.version = newVersion;
  // 保留原有换行符风格 / Preserve original line endings
  const newlineChar = raw.includes("\r\n") ? "\r\n" : "\n";
  const output = JSON.stringify(obj, null, indent).replace(/\n/g, newlineChar) + newlineChar;
  fs.writeFileSync(filePath, output, "utf8");
  return oldVersion;
}

/**
 * 粗略检测 JSON 文件的缩进 —— 取第一个缩进行的前导空白。
 * Detect JSON indentation by inspecting the first indented line.
 */
function detectJsonIndent(raw) {
  for (const line of raw.split(/\r?\n/)) {
    const m = line.match(/^(\t| {1,8})\S/);
    if (m) return m[1] === "\t" ? "\t" : m[1].length;
  }
  return "\t"; // fallback
}

/**
 * 更新 Cargo.toml [package] 段的 version 字段，逐行替换，保留其余内容不变。
 * Update the version field in Cargo.toml's [package] section line-by-line.
 */
function patchCargoToml(filePath, newVersion) {
  const raw = fs.readFileSync(filePath, "utf8");
  let inPackage = false;
  let replaced = false;
  let oldVersion = null;

  const lines = raw.split(/\r?\n/);
  const out = lines.map((line) => {
    const trimmed = line.trim();
    // 检测段落标题 / Detect section headers
    if (/^\[.*\]/.test(trimmed)) {
      inPackage = trimmed === "[package]";
    }
    // 仅在 [package] 段内替换 version，且只替换第一次，避免误改依赖版本
    // Only replace version within [package], and only the first occurrence
    if (inPackage && !replaced && /^version\s*=\s*"[^"]+"/.test(trimmed)) {
      oldVersion = trimmed.match(/^version\s*=\s*"([^"]+)"/)[1];
      replaced = true;
      return line.replace(/^(\s*version\s*=\s*)"[^"]+"/, `$1"${newVersion}"`);
    }
    return line;
  });

  const newlineChar = raw.includes("\r\n") ? "\r\n" : "\n";
  fs.writeFileSync(filePath, out.join(newlineChar), "utf8");
  return oldVersion;
}

// ── 主流程 / Main ─────────────────────────────────────────────────────────────

const newVersion = process.argv[2];

if (!newVersion || newVersion === "--help" || newVersion === "-h") {
  console.log(`\n${C.bold}用法 / Usage:${C.reset}`);
  console.log("  node scripts/bump-version.js <new-version>");
  console.log("  npm run bump-version -- <new-version>\n");
  console.log(`${C.bold}示例 / Example:${C.reset}`);
  console.log("  node scripts/bump-version.js 0.4.0\n");
  process.exit(newVersion ? 0 : 1);
}

validateVersion(newVersion);

const ROOT = path.resolve(__dirname, "..");

// 需要更新的文件列表 / List of files to update
// 每项: { rel: 相对路径, type: "json" | "cargo", label: 显示名 }
const targets = [
  { rel: "package.json",                      type: "json",  label: "package.json (root workspace)" },
  { rel: "frontend/package.json",             type: "json",  label: "frontend/package.json" },
  { rel: "backend/Cargo.toml",                type: "cargo", label: "backend/Cargo.toml" },
  { rel: "desktop/package.json",              type: "json",  label: "desktop/package.json" },
  { rel: "desktop/src-tauri/Cargo.toml",      type: "cargo", label: "desktop/src-tauri/Cargo.toml" },
  { rel: "desktop/src-tauri/tauri.conf.json", type: "json",  label: "desktop/src-tauri/tauri.conf.json" },
];

console.log(`\n${C.cyan}${"═".repeat(60)}${C.reset}`);
console.log(`${C.bold}  Bump version → ${newVersion}${C.reset}`);
console.log(`${C.cyan}${"═".repeat(60)}${C.reset}\n`);

let anyError = false;

for (const t of targets) {
  const filePath = path.join(ROOT, t.rel);
  if (!fs.existsSync(filePath)) {
    warn(`${t.label}  ${C.gray}(文件不存在，已跳过 / file not found, skipped)${C.reset}`);
    continue;
  }
  try {
    let oldVersion;
    if (t.type === "json") {
      oldVersion = patchJson(filePath, newVersion);
    } else {
      oldVersion = patchCargoToml(filePath, newVersion);
    }
    const change = oldVersion && oldVersion !== newVersion
      ? `${C.gray}${oldVersion}${C.reset} → ${C.bold}${newVersion}${C.reset}`
      : `${C.bold}${newVersion}${C.reset} ${C.gray}(unchanged)${C.reset}`;
    ok(`${t.label}  ${change}`);
  } catch (e) {
    fail(`${t.label}  ${e.message}`);
    anyError = true;
  }
}

console.log();

if (anyError) {
  fail("部分文件更新失败，请检查上方错误。");
  fail("Some files failed to update; see errors above.");
  process.exit(1);
}

console.log(`${C.green}${C.bold}  All done!${C.reset}  版本号已统一更新为 ${C.bold}${newVersion}${C.reset}\n`);
console.log(`${C.gray}  提示 / Tip: 记得提交所有变更后再构建或推送。${C.reset}`);
console.log(`${C.gray}  Tip: commit all changes before building or pushing.${C.reset}\n`);
