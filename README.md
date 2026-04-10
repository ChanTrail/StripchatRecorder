# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

自托管的 Stripchat 直播录制工具，提供基于 Web 的管理界面，支持自动录制、后处理流水线和多渠道通知。

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## 功能特性

- 监控多个主播，上线时自动开始录制
- Web UI 管理主播、录制文件和后处理任务
- **主播查找**：通过 [camgirlfinder.net](https://camgirlfinder.net) 查找主播，支持：
  - 人脸识别搜索：上传图片自动检测人脸，找出相似主播
  - 名字搜索：按用户名关键词搜索
  - 从结果卡片直接一键添加到录制列表
  - 对任意主播发起相似人脸搜索
- 支持分离式网络代理：可分别配置 Stripchat API 代理与 CDN 分片代理
- 支持配置 Stripchat 镜像站（将请求中的 `stripchat.com` 替换为镜像域名）
- 可配置的后处理流水线，支持插件化模块：
  - **contact_sheet** — 生成带时间戳的缩略图预览图
  - **filter_short** — 删除低于最短时长的录制文件
  - **notify_discord** — 通过 Discord Webhook 发送录制信息和封面图
  - **notify_telegram** — 通过 MTProto 发送录制信息、封面图和视频（支持超过 2 GB 的大文件，支持 HTTP/SOCKS5 代理）
- 双运行模式：可作为 Tauri 桌面应用或无头服务器通过浏览器访问
- 基于 SSE 的实时 UI 更新
- 跟随系统主题的深色/浅色模式
- 支持自定义界面语言，详见[自定义语言文档](docs/custom-locale.md)

---

## 快速开始（Docker）

### docker-compose（推荐）

```yaml
services:
  stripchat-recorder:
    image: chantrail/stripchat-recorder:latest
    container_name: stripchat-recorder
    restart: unless-stopped
    environment:
      - TZ=Asia/Shanghai
      # - LANGUAGE=en-US  # 设置界面语言，支持 zh-CN（默认）或 en-US
      # - PORT=3030        # 设置服务端口（默认 3030）
    ports:
      - "${PORT:-3030}:${PORT:-3030}"
    volumes:
      - ./data/logs:/app/stripchat-recorder/logs
      - ./data/recordings:/app/stripchat-recorder/recordings
      - ./data/modules:/app/stripchat-recorder/modules
      - ./data/config:/app/stripchat-recorder/config
```

```bash
docker compose up -d
```

启动后在浏览器中打开 `http://localhost:3030`。

Docker 镜像默认以 Server 模式运行（端口 3030），配置写入挂载的 `config/settings.json`。

### 网络代理与镜像站

在设置页的「网络代理」中可分别配置：

Stripchat 镜像站项目：<https://github.com/ChanTrail/StripchatMirror>

1. API 代理：用于访问 Stripchat API；若同时填写镜像站，则通过该代理访问镜像站。
2. CDN 代理：用于下载直播分片流，可与 API 代理分开设置。
3. Stripchat 镜像站：用于替换请求中的 `stripchat.com` 域名。

### docker run

```bash
docker run -d \
  --name stripchat-recorder \
  --restart unless-stopped \
  -e TZ=Asia/Shanghai \
  -e LANGUAGE=en-US \
  -e PORT=3030 \
  -p 3030:3030 \
  -v ./data/logs:/app/stripchat-recorder/logs \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/config:/app/stripchat-recorder/config \
  chantrail/stripchat-recorder:latest
```

---

## 后处理模块

模块是实现了简单协议的独立可执行文件，通过环境变量接收输入，通过标准输出与主程序通信。

### 内置模块

| 模块              | 说明                                                |
| ----------------- | --------------------------------------------------- |
| `contact_sheet`   | 按配置间隔截帧并拼合为预览图                        |
| `filter_short`    | 删除低于最短时长的录制文件                          |
| `notify_discord`  | 通过 Discord Webhook 发送录制信息和封面图           |
| `notify_telegram` | 通过 MTProto 向 Telegram 发送录制信息、封面图和视频 |

将自定义模块放入 `modules` 数据卷目录后会被自动发现，且不会在容器重启时被覆盖。详见[后处理模块开发文档](docs/module-development.md)。

---

## 从源码构建

**前置依赖：** Rust、Node.js (LTS)、ffmpeg

### 首次启动配置

直接运行二进制文件时，若 `config/settings.json` 中尚未配置运行模式，会自动进入命令行 TUI 引导配置：

1. 选择界面语言（中文 / English）
2. 选择运行模式（Desktop 桌面端 / Server 服务器端）
3. Server 模式下输入监听端口（默认 3030）

配置完成后写入 `config/settings.json`，下次启动直接读取，不再弹出配置界面。

```bash
# 安装前端依赖
npm install

# 构建前端 + Tauri 二进制
npm run build
npx tauri build --no-bundle

# 构建后处理模块
for dir in modules/*/; do
  [ -f "$dir/Cargo.toml" ] && cargo build --manifest-path "$dir/Cargo.toml" --release --bins
done
```

### 构建 Docker 镜像

```bash
docker build -t chantrail/stripchat-recorder .
```

---

## 技术栈

- **前端：** Vue 3, TypeScript, Vite, Tailwind CSS, Reka UI
- **后端 / 桌面端：** Rust, Tauri 2
- **后处理模块：** Rust（独立二进制）
- **容器：** Debian, ffmpeg

---

## 开源许可证

本项目基于 [GNU 通用公共许可证 v3.0](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html) 发布。

---

## 免责声明

本项目仅用于技术研究与学习交流。使用者需自行承担部署、运维与合规风险。
