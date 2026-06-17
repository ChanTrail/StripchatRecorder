# StripchatRecorder Desktop

基于 [Tauri](https://tauri.app) 的桌面应用版本，复用 `backend/` 的核心录制逻辑。

## 与 Server 模式的主要差异

| 功能 | Server 模式 | Desktop 模式 |
|------|-------------|--------------|
| 通信层 | HTTP REST + SSE | Tauri IPC (invoke/listen) |
| 配置目录 | exe 同目录 `config/` | 系统 App Data 目录 |
| 流转发 | ✅ 支持 | ❌ 不支持 |
| 前端访问 | 浏览器 | Tauri WebView |

## 目录结构

```
desktop/
├── src/                  Vue 3 前端（基于 frontend/ 修改）
│   ├── lib/api.ts        ← 替换为 Tauri invoke/listen
│   ├── i18n.ts           ← 替换为 Tauri invoke
│   └── router/index.ts   ← 移除 relay 路由
├── src-tauri/            Tauri Rust 后端
│   └── src/
│       ├── commands.rs   Tauri command 层（对应 server 的 REST 路由）
│       ├── emitter.rs    TauriEmitter（实现 backend Emitter trait）
│       ├── lib.rs        应用初始化入口
│       └── state.rs      全局状态结构
├── vite.config.ts
└── package.json
```

## 开发环境准备

- Rust + Cargo
- Node.js + npm
- [Tauri 系统依赖](https://tauri.app/start/prerequisites/)（Windows 需要 WebView2，通常已预装）
- ffmpeg（需在 PATH 中，用于录制和后处理）

## 开发运行

```bash
cd desktop
npm install
npm run tauri dev
```

## 构建发布

```bash
cd desktop
npm run tauri build
```

构建产物在 `src-tauri/target/release/bundle/` 下，包括：
- `nsis/` — Windows 安装包（.exe）
- `msi/` — Windows MSI 安装包

## 配置数据目录

Desktop 模式使用系统 App Data 目录存储配置：
- Windows: `%APPDATA%\com.chantrail.stripchat-recorder\`
- macOS: `~/Library/Application Support/com.chantrail.stripchat-recorder/`
- Linux: `~/.config/com.chantrail.stripchat-recorder/`

目录下的结构与 server 模式相同：
```
<app-data>/
├── config/         设置、主播列表、后处理配置
├── locale/         语言文件
├── logs/           日志文件
├── modules/        后处理模块（.exe）
└── recordings/     默认录制输出目录（可在设置中修改）
```

## 图标

`src-tauri/icons/` 中当前为占位图标，正式发布前需替换：

```bash
# 使用 tauri CLI 从单张 1024x1024 PNG 自动生成所有尺寸的图标
npm run tauri icon path/to/your-icon.png
```
