#!/usr/bin/env node
/**
 * Desktop Release 构建 / Desktop release build
 *
 * 1. 类型检查（vue-tsc --noEmit）+ 所有模块 cargo check
 * 2. 安装 desktop 依赖
 * 3. 构建所有模块（release），复制二进制到 build_tmp/desktop/target/release/modules/
 * 4. Tauri 构建（tauri build）
 *    - 前端由 Tauri 的 beforeBuildCommand 自动执行 vite build → desktop/dist/
 *    - Rust 编译产物重定向到 build_tmp/desktop/target/
 * 5. 收集 bundle 产物 → build/desktop/
 * 6. 收集模块二进制 → build/desktop/modules/
 * 7. 清理 build_tmp/desktop/
 *
 * Usage: npm run build:desktop
 */

"use strict";

const path = require("path");
const fs = require("fs");

const {
	ROOT,
	DESKTOP,
	DESKTOP_TARGET,
	BUILD_OUT,
	BUILD_TMP,
	NESTED,
	step,
	header,
	run,
	listDir,
	collectBinaries,
	buildModules,
	copyDir,
	installDesktop,
	readDesktopIdentifier,
} = require("./common");

// 平台标识符：优先读环境变量（跨平台 CI 交叉打包时使用），否则按当前宿主机推导。
// 必须传给 buildModules，否则复制出的文件名只有 {name}-{version} 两段，
// 会被后端 discovery.rs 的 has_valid_module_filename（要求 ≥3 段）过滤掉，
// 导致模块列表为空（此前 desktop release 构建一直存在这个问题）。
//
// Platform identifier: prefer env var (used for cross-platform CI packaging),
// else detect from the current host. Must be passed to buildModules — otherwise
// copied filenames only have the {name}-{version} two segments, which get filtered
// out by the backend's has_valid_module_filename (requires ≥3 segments), resulting
// in an empty module list (this bug has been present in desktop release builds).
function detectPlatform() {
	const archStr = process.arch === "arm64" ? "aarch64" : "x86_64";
	if (process.platform === "win32") return `windows-${archStr}`;
	if (process.platform === "darwin") return `darwin-${archStr}`;
	return `linux-${archStr}`;
}
const platform = process.env.CARGO_BUILD_PLATFORM || detectPlatform();

/** Tauri bundle 产物源目录 / Tauri bundle output source */
const TAURI_BUNDLE_SRC = path.join(DESKTOP_TARGET, "release", "bundle");

/** Desktop 产物收集目标目录 / Desktop artifact collection target */
const DESKTOP_BUILD_OUT = path.join(BUILD_OUT, "desktop");

/** desktop build_tmp 目录 / Desktop build_tmp directory */
const DESKTOP_BUILD_TMP = path.join(BUILD_TMP, "desktop");

const TOTAL = 7;
header(
	"Desktop Build",
	"check → install → modules → tauri build → collect → collect modules → cleanup",
);

// ── Step 1: 类型检查 / Type check ────────────────────────────────────────────
step(1, TOTAL, "Type checking desktop + modules");
run("node scripts/check.desktop.js", {
	cwd: ROOT,
	env: { ...process.env, CHECK_NESTED: "1" },
});

// ── Step 2: 安装依赖 / Install dependencies ──────────────────────────────────
step(2, TOTAL, "Installing desktop dependencies");
if (fs.existsSync(DESKTOP_BUILD_OUT))
	fs.rmSync(DESKTOP_BUILD_OUT, { recursive: true, force: true });
installDesktop();

// ── Step 3: 构建模块并复制 / Build modules & copy binaries ───────────────────
step(3, TOTAL, "Building modules (release) → build/desktop/modules/");
const DESKTOP_MODULES_OUT = path.join(DESKTOP_BUILD_OUT, "modules");
buildModules("release", DESKTOP_MODULES_OUT, null, platform);

// ── Step 4: Tauri 构建 / Tauri build ─────────────────────────────────────────
// TAURI_BUILD_VARIANTS：分号分隔的多组 tauri build 参数，每组触发一次
// `npx tauri build <args>`；字面量 "default" 表示无参默认构建。缺省只构建一次
// （原行为）。用于同平台产出不同变体，例如 Windows 安装包 + 便携版：
//   "--bundles nsis;--no-bundle -- --features portable"
// Linux 则借它让 AppImage 单独走 portable feature（deb 保持安装版）：
//   "--bundles deb;--bundles appimage -- --features portable"
//
// TAURI_BUILD_VARIANTS: semicolon-separated sets of `tauri build` args, each
// triggering one `npx tauri build <args>`; the literal "default" means a plain
// no-arg build. Defaults to a single build (the old behavior). Lets one
// platform emit multiple variants, e.g. Windows installer + portable:
//   "--bundles nsis;--no-bundle -- --features portable"
// On Linux this builds the AppImage alone with the portable feature while deb
// stays an installed build:
//   "--bundles deb;--bundles appimage -- --features portable"
const variants = (process.env.TAURI_BUILD_VARIANTS || "default")
	.split(";")
	.map((s) => s.trim())
	.filter(Boolean);
step(4, TOTAL, `Building desktop (tauri build ×${variants.length}) → build_tmp/desktop/target/`);
for (const variant of variants) {
	const args = variant === "default" ? "" : ` ${variant}`;
	console.log(`  → npx tauri build${args || " (default)"}`);
	run(`npx tauri build${args}`, {
		cwd: DESKTOP,
		env: { ...process.env, CARGO_TARGET_DIR: DESKTOP_TARGET },
	});

	// --no-bundle 变体构建后立即收集裸二进制到 build/desktop/portable/
	// （在循环内按参数判断，与变体顺序无关；cleanup 删 build_tmp 之前完成）。
	// DESKTOP_COLLECT_NO_BUNDLE=1 时启用。二进制名 = Cargo 包名。
	//
	// Right after a --no-bundle variant, collect the bare binary into
	// build/desktop/portable/ (detected from the args inside the loop, so variant
	// order doesn't matter; still before cleanup removes build_tmp). Enabled by
	// DESKTOP_COLLECT_NO_BUNDLE=1. Binary name = the Cargo package name.
	if (args.includes("--no-bundle") && process.env.DESKTOP_COLLECT_NO_BUNDLE === "1") {
		const cargoToml = fs.readFileSync(path.join(DESKTOP, "src-tauri", "Cargo.toml"), "utf8");
		const pkgName = (cargoToml.match(/^\s*name\s*=\s*"([^"]+)"/m) || [])[1];
		const binName = `${pkgName}${process.platform === "win32" ? ".exe" : ""}`;
		const binSrc = path.join(DESKTOP_TARGET, "release", binName);
		const portableOut = path.join(DESKTOP_BUILD_OUT, "portable");
		fs.mkdirSync(portableOut, { recursive: true });
		fs.copyFileSync(binSrc, path.join(portableOut, binName));
		console.log(`  ✓ build/desktop/portable/${binName}`);
	}
}

// ── Step 5: 收集 bundle 产物 / Collect bundle artifacts ──────────────────────
step(5, TOTAL, "Collecting bundle artifacts → build/desktop/");

if (!fs.existsSync(TAURI_BUNDLE_SRC)) {
	console.error(`ERROR: Tauri bundle directory not found: ${TAURI_BUNDLE_SRC}`);
	process.exit(1);
}
copyDir(TAURI_BUNDLE_SRC, DESKTOP_BUILD_OUT, "build/desktop/");

// ── Step 6: 收集模块二进制 / Collect module binaries ─────────────────────────
step(6, TOTAL, "Collecting module binaries → build/desktop/modules/");
const moduleBins = collectBinaries(DESKTOP_MODULES_OUT);
if (moduleBins.length === 0) {
	console.warn("  ⚠ No module binaries found");
} else {
	for (const bin of moduleBins) {
		console.log(`  ✓ build/desktop/modules/${bin}`);
	}
}

// 用户安装打包产物后，模块二进制需要手动放到 Tauri app_data_dir()（每用户标准数据
// 目录，见 backend/src/config/app_state.rs::set_exe_dir_override）下的 modules/ 子
// 目录 —— 不再是可执行文件所在目录（安装包场景下该目录常只读/无权限/属于签名产物，
// 详见该文档注释）。三平台的具体路径见 README。
//
// After installing the packaged app, module binaries must be placed manually under
// the modules/ subdirectory of Tauri's app_data_dir() (the OS-standard per-user data
// directory, see set_exe_dir_override in backend/src/config/app_state.rs) — no longer
// the executable's directory (which, for installer packages, is often read-only,
// unwritable, or part of a signed artifact; see that doc comment for details). See
// README for the exact per-platform path.
const desktopIdentifier = readDesktopIdentifier();
console.log("\n  ⚠ Module install path (per-user data directory, not next to the executable):");
console.log(`      Windows: %APPDATA%\\${desktopIdentifier}\\modules\\`);
console.log(`      macOS:   ~/Library/Application Support/${desktopIdentifier}/modules/`);
console.log(`      Linux:   ~/.local/share/${desktopIdentifier}/modules/  (or $XDG_DATA_HOME)`);

// ── Step 7: 清理 / Cleanup ───────────────────────────────────────────────────
step(7, TOTAL, "Cleanup");
fs.rmSync(DESKTOP_BUILD_TMP, { recursive: true, force: true });
console.log("  ✓ build_tmp/desktop/ removed");

// ── 完成 / Done ──────────────────────────────────────────────────────────────
console.log(`\n${"═".repeat(60)}`);
console.log("  Desktop build complete!");
console.log("  Output: build/desktop/");
console.log("═".repeat(60));
listDir(DESKTOP_BUILD_OUT);
