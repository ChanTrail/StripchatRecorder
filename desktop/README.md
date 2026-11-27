# StripchatRecorder Desktop

基于 [Tauri](https://tauri.app) 的桌面应用版本，复用 `backend/` 的核心录制逻辑。

## 与 Server 模式的主要差异

| 功能     | Server 模式          | Desktop 模式                                                            |
| -------- | -------------------- | ----------------------------------------------------------------------- |
| 通信层   | HTTP REST + SSE      | Tauri IPC (invoke/listen)                                               |
| 配置目录 | exe 同目录 `config/` | 系统 App Data 目录（便携版/AppImage 随程序本体）                        |
| 流转发   | ✅ 支持              | ✅ 内置按需转发——无管理页，浏览页/操作直接唤起本地播放器（可自定义）    |
| 前端访问 | 浏览器               | Tauri WebView                                                           |

## 目录结构

```
desktop/
├── src/                  Vue 3 前端（基于 frontend/ 修改）
│   ├── lib/api.ts        ← 替换为 Tauri invoke/listen
│   ├── i18n.ts           ← 替换为 Tauri invoke
│   └── router/index.ts   ← 无 relay/登录路由（转发走内置按需监听 + 本地播放器，无管理页）
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
- Node.js + [pnpm](https://pnpm.io/installation)（版本见 `package.json` 的 `packageManager`）
- [Tauri 系统依赖](https://tauri.app/start/prerequisites/)（Windows 需要 WebView2，通常已预装）
- ffmpeg（可选：PATH 与数据目录 `tools/` 都缺失时应用启动后自动下载，构建轨与形态可在向导中选择）

## 开发运行

```bash
cd desktop
pnpm install
pnpm tauri dev
```

## 构建发布

```bash
cd desktop
pnpm tauri build
```

构建产物在 `src-tauri/target/release/bundle/` 下，包括：

- `nsis/` — Windows 安装包（.exe）
- `msi/` — Windows MSI 安装包

## 配置数据目录

Desktop 安装版使用系统 App Data 目录存储配置：

- Windows: `%APPDATA%\com.chantrail.stripchat-recorder\`
- macOS: `~/Library/Application Support/com.chantrail.stripchat-recorder/`
- Linux: `~/.local/share/com.chantrail.stripchat-recorder/`（或 `$XDG_DATA_HOME`）

目录下的结构与 server 模式相同：

```
<app-data>/
├── config/         设置、主播列表、后处理配置
├── locale/         语言文件
├── logs/           日志文件
├── modules/        后处理模块（.exe）
├── tools/          FFmpeg（程序自动下载或自备构建）
└── recordings/     默认录制输出目录（可在设置中修改）
```

**便携版（Windows `*-portable.zip`、Linux AppImage）**：数据目录跟随程序本体——`config/`、`modules/`、`tools/` 等都在可执行文件旁边。首次运行时若旧安装版的每用户数据目录里已有 `config/`、`locale/`、`modules/`，会自动迁移过来（仅当便携目录中对应目录不存在）。

## 转发流（Relay）

Desktop 没有独立的转发流管理页，但内置了按需转发监听：首次在 StripChat 浏览页/转发操作请求播放时，后端惰性启动本地 `http://127.0.0.1:{port}/stream/{username}` HTTP-FLV 监听并用播放器打开——播放器走设置项「自定义播放器」（`player_path`），留空则自动探测 mpv/VLC/PotPlayer 等。

## 图标

`src-tauri/icons/` 中当前为占位图标，正式发布前需替换：

```bash
# 使用 tauri CLI 从单张 1024x1024 PNG 自动生成所有尺寸的图标
pnpm tauri icon path/to/your-icon.png
```
