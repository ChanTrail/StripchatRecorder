# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

自托管的 Stripchat 直播录制工具，提供基于 Web 的管理界面，支持自动录制、后处理流水线和多渠道通知。

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## 功能特性

- 监控多个主播，上线时自动开始录制
- Web UI 管理主播、录制文件和后处理任务
- **管理员认证**：密码保护管理界面，初次使用时通过设置向导创建密码；Token 绑定登录 IP，8 小时有效，支持最多 5 个并发会话
- **主播查找**：通过 [camgirlfinder.net](https://camgirlfinder.net) 查找主播，支持：
  - 人脸识别搜索：上传图片自动检测人脸，找出相似主播
  - 名字搜索：按用户名关键词搜索
  - 从结果卡片直接一键添加到录制列表
  - 对任意主播发起相似人脸搜索
- **转发流（HLS Relay）**：无需录制即可将主播直播流转发给播放器，访问 `/stream/{modelname}` 自动启动，支持多客户端同时连接
- 支持分离式网络代理：可分别配置 Stripchat API 代理与 CDN 分片代理
- 支持配置 Stripchat 镜像站（将请求中的 `stripchat.com` 替换为镜像域名）
- **Mouflon HLS 解密**：支持管理 `pkey → pdkey` 密钥对，用于解密 Stripchat 加密的 HLS 分片文件名；支持配置同步 URL 自动拉取最新密钥
- **可视化后处理流水线**：DAG 节点编辑器，支持分叉与合并，内置模块：
  - **ts_merge** — 将 TS 分片目录合并为单一视频文件（流水线首节点）
  - **contact_sheet** — 生成带时间戳的缩略图预览图
  - **filter_short** — 删除低于最短时长的录制文件
  - **notify_discord** — 通过 Discord Webhook 发送录制信息和封面图
  - **notify_telegram** — 通过 MTProto 发送录制信息、封面图和视频（支持超过 2 GB 的大文件，支持 HTTP/SOCKS5 代理）
  - **cleanup** — 后处理完成后清理录制相关文件及临时缓存
- **社区模块市场**：在 Web UI 中浏览、安装、更新社区贡献的后处理模块，支持配置下载代理和加速镜像
- **关于页面 / 自动更新**：检查 GitHub Releases 新版本；非 Docker 环境支持一键下载并自动替换程序文件；Docker 环境提示手动更新镜像
- 录制文件页磁盘空间监控，剩余空间不足 5 GB 时高亮提示
- 双运行模式：可作为 Tauri 桌面应用或无头服务器通过浏览器访问
- 基于 SSE 的实时 UI 更新，支持多客户端同步
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

启动后在浏览器中打开 `http://localhost:3030`。首次访问时会进入设置向导，引导你完成语言选择、录制目录设置、网络代理配置和管理员密码设置。

Docker 镜像默认以 Server 模式运行（端口 3030），配置写入挂载的 `config/settings.json`。

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

## 主要设置项

在 Web UI 的「设置」页面可配置以下选项：

| 设置项                        | 说明                                                                                   |
| ----------------------------- | -------------------------------------------------------------------------------------- |
| TS 流输出目录                 | TS 分片流存放路径（录制产生的原始分片）                                                |
| 最大并发录制数                | 同时录制的最大主播数，`0` 表示不限制                                                   |
| 后处理最大并发数              | 同时运行的后处理任务数，`0` 表示自动（= CPU 逻辑核心数）                               |
| 轮询间隔（秒）                | 检查主播是否上线的间隔，范围 10–300                                                    |
| 首选录制分辨率                | 目标录制画质（0 = 原始/最高画质），可设置画质不可用时的回退方向                        |
| 录制文件时长（秒）            | 每个分片文件的最长时长，`0` 表示不限制；可用于按时段切割长播录制                       |
| 上线自动录制                  | 新添加的主播是否默认开启自动录制                                                       |
| 后处理临时目录最大占用（GB）  | 后处理模块运行时产生的临时文件上限，超出后自动删除最旧的文件，`0` 表示不限制，默认 50 GB |
| 检查 beta 版本更新            | 是否同时检查预发布（beta/rc）版本；beta 版本构建自动强制开启                           |

### 网络代理与镜像站

在设置页的「网络」中可分别配置：

Stripchat 镜像站项目：<https://github.com/ChanTrail/StripchatMirror>

1. **API 代理**：用于访问 Stripchat API；若同时填写镜像站，则通过该代理访问镜像站。
2. **CDN 代理**：用于下载直播分片流，可与 API 代理分开设置。
3. **Stripchat 镜像站**：用于替换请求中的 `stripchat.com` 域名。
4. **社区模块代理**：用于下载社区模块文件，可独立配置。
5. **社区模块加速镜像**：在 GitHub 地址前加前缀（如 `https://ghproxy.com`），加速模块下载。

### 管理员认证

首次访问时设置向导会引导创建管理员密码（至少 6 位，需包含字母、数字和特殊字符）。之后每次访问需要登录，Token 绑定登录 IP，有效期 8 小时，每次请求自动续期，最多支持 5 个并发登录会话。

在设置页的「安全」分区可以修改密码。

### Mouflon HLS 解密密钥

Stripchat 对 HLS 分片文件名进行了加密（Mouflon 系统）。若录制时遇到无法下载分片的情况，需在设置页的「Mouflon 解密密钥」中填入对应的 `pkey → pdkey` 密钥对。密钥可从社区渠道获取。

也可以配置「同步地址」和可选的「同步令牌」，从指定 URL 自动拉取最新密钥。

### 转发流（HLS Relay）

在 Server 模式下，无需将主播添加到录制列表，直接用播放器打开以下地址即可播放直播：

```
http://localhost:3030/stream/{modelname}
```

首次访问时自动连接上游，支持多个客户端同时连接同一转发流。在 Web UI 的「转发流」页面可查看所有活跃会话的状态、连接数和运行时长。

---

## 后处理模块

后处理流水线是一个 **DAG（有向无环图）**，在可视化编辑器中拖拽连线即可构建处理链路，支持分叉与合并。

### 内置模块

| 模块              | 说明                                                              |
| ----------------- | ----------------------------------------------------------------- |
| `ts_merge`        | 将 TS 分片目录合并为单一视频文件，**官方流水线的首节点**         |
| `contact_sheet`   | 按配置间隔截帧并拼合为预览图                                      |
| `filter_short`    | 删除低于最短时长的录制文件                                        |
| `notify_discord`  | 通过 Discord Webhook 发送录制信息和封面图                         |
| `notify_telegram` | 通过 MTProto 向 Telegram 发送录制信息、封面图和视频               |
| `cleanup`         | 清理录制相关文件及其他模块产生的临时缓存，通常作为流水线最后一个节点 |

### 社区模块市场

在 Web UI 的「模块社区」页面可以浏览、安装和更新由社区贡献的第三方后处理模块。安装前会展示免责声明；支持通过设置页配置下载代理和加速镜像。

自定义模块放入 `modules` 数据卷目录后会被自动发现，且不会在容器重启时被覆盖。详见[后处理模块开发文档](docs/module-development.md)。

> **文件名格式：** 主程序只加载文件名符合 `{name}-{platform}-{version}` 格式的可执行文件。

### Desktop 端模块安装路径

Desktop 端安装包（NSIS/MSI、AppImage、deb/rpm、dmg）不把模块放在程序安装目录下。请将下载的 `modules-{platform}.zip` 解压到每用户数据目录的 `modules` 子目录：

| 平台    | 路径                                                                              |
| ------- | --------------------------------------------------------------------------------- |
| Windows | `%APPDATA%\com.chantrail.stripchat-recorder\modules\`                             |
| macOS   | `~/Library/Application Support/com.chantrail.stripchat-recorder/modules/`         |
| Linux   | `~/.local/share/com.chantrail.stripchat-recorder/modules/`（或 `$XDG_DATA_HOME`）|

目录不存在时可手动创建；应用启动后会自动监控该目录，新增/删除模块无需重启。

---

## 从源码构建

**前置依赖：** Rust、Node.js (LTS)、ffmpeg

### 首次启动配置

直接运行二进制文件时，若 `config/settings.json` 中尚未完成向导配置，会自动进入 Web 设置向导：

1. 选择界面语言
2. 设置录制输出目录
3. 配置网络代理（可选）
4. 设置管理员密码

配置完成后写入 `config/settings.json`，下次启动直接读取，不再弹出配置向导。

```bash
# 安装前端依赖
npm install

# 构建 Server 前端 + 后端
npm run build

# 构建 Desktop 版本
npm run build:desktop

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
