# StripchatRecorder

> 🌐 [English](README.en.md)

自托管的 Stripchat 直播录制工具，提供基于 Web 的管理界面，支持自动录制、后处理流水线和多渠道通知。

[![License: GPL-2.0](https://img.shields.io/badge/License-GPL--2.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-2.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## 功能特性

- 监控多个主播，上线时自动开始录制
- Web UI 管理主播、录制文件和后处理任务
- 可配置的后处理流水线，支持插件化模块：
  - **contact_sheet** — 生成带时间戳的缩略图预览图
  - **filter_short** — 删除低于最短时长的录制文件
  - **notify_discord** — 通过 Discord Webhook 发送录制信息和封面图
  - **notify_telegram** — 通过 MTProto 发送录制信息、封面图和视频（支持超过 2 GB 的大文件，支持 HTTP/SOCKS5 代理）
- 双运行模式：可作为 Tauri 桌面应用或无头服务器通过浏览器访问
- 基于 SSE 的实时 UI 更新
- 跟随系统主题的深色/浅色模式

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
    ports:
      - "3030:3030"
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

### docker run

```bash
docker run -d \
  --name stripchat-recorder \
  --restart unless-stopped \
  -e TZ=Asia/Shanghai \
  -p 3030:3030 \
  -v ./data/logs:/app/stripchat-recorder/logs \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/config:/app/stripchat-recorder/config \
  chantrail/stripchat-recorder:latest
```

## 后处理模块

模块是实现了简单协议的独立可执行文件，通过环境变量接收输入，通过标准输出与主程序通信。

### 内置模块

| 模块 | 说明 |
|------|------|
| `contact_sheet` | 按配置间隔截帧并拼合为预览图 |
| `filter_short` | 删除低于最短时长的录制文件 |
| `notify_discord` | 通过 Discord Webhook 发送录制信息和封面图 |
| `notify_telegram` | 通过 MTProto 向 Telegram 发送录制信息、封面图和视频 |

将自定义模块放入 `modules` 数据卷目录后会被自动发现，且不会在容器重启时被覆盖。详见[后处理模块开发文档](docs/module-development.md)。

---

## 从源码构建

**前置依赖：** Rust、Node.js (LTS)、ffmpeg

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

本项目基于 [GNU 通用公共许可证 v2.0](https://www.gnu.org/licenses/old-licenses/gpl-2.0.html) 发布。
