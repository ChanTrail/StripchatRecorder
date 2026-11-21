# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

自托管的 Stripchat 直播录制工具，提供基于 Web 的管理界面，支持自动录制、后处理流水线和多渠道通知。

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)
[![GHCR](https://img.shields.io/badge/ghcr.io-chantrail%2Fstripchat--recorder-blue)](https://github.com/ChanTrail/StripchatRecorder/pkgs/container/stripchat-recorder)

---

## 功能特性

- 监控多个主播，上线时自动开始录制
- Web UI 管理主播、录制文件和后处理任务
- **管理员认证**：密码保护管理界面，初次使用时通过设置向导创建密码；Token 绑定登录 IP，8 小时有效，支持最多 5 个并发会话
- **主播查找**：通过 [camgirlfinder.net](https://camgirlfinder.net) 查找主播，支持：
    - 人脸识别搜索：上传图片或填入图片 URL 自动检测人脸，找出相似主播；可选背景图模式展示完整结果图
    - 名字搜索：按用户名关键词搜索
    - 两种模式结果互转：从名字搜索结果直接发起该主播的人脸搜索，或从人脸搜索结果转查主播名
    - 从结果卡片直接一键添加到录制列表
- **改名跟踪**：添加主播时通过用户 ID API 获取并持久化 Stripchat 内部 ID；后续轮询直接用 ID 查询状态，可在主播改名时自动检测并更新记录，无需手动重新添加
- **StripChat 浏览页**：按 Stripchat 官方分类（女生/情侣/男生/跨性别）浏览在线直播，支持特征与语言标签筛选、排序与无限滚动，从卡片一键添加主播到追踪列表；卡片徽标提示互动玩具（TOY）等特性；卡片状态按轮询间隔静默回刷；桌面端点击卡片自动经内置转发流唤起本地播放器（自定义播放器或自动探测 mpv/VLC/PotPlayer/IINA 等）直接播放；列表与快照请求走独立的代理与镜像站设置
- **隐私模式**：一键模糊预览图避免走光，三档可选（关闭 / 仅直播画面 / 全部图片），主播列表页与 StripChat 浏览页各自独立开关，单张图片可单独解除模糊
- **转发流（HLS Relay）**：无需录制即可将主播直播流转发给播放器，访问 `/stream/{modelname}` 自动启动，支持多客户端同时连接；可用 `?resolution=&codec=&preference=` 查询参数临时覆盖流选择
- 支持分离式网络代理：可分别配置 Stripchat API 代理、CDN 分片代理、浏览页代理与 camgirlfinder 代理
- 支持配置 Stripchat 镜像站（将请求中的 `stripchat.com` 替换为镜像域名，可选 http/https 协议），浏览页镜像站可独立配置
- **ECH（加密 ClientHello）**：可选开启，StripChat API 请求的 TLS ClientHello 经借用 ECH 配置加密以隐藏真实 SNI，可能改善 API 返回 403 的情况；不可路由或解析失败时自动回退普通连接，CDN 分片仅在直连全部失败后的兜底竞速中带 ECH 腿
- **Mouflon HLS 解密**：支持管理 `pkey → pdkey` 密钥对和 `pkey → keystream` 密钥流，用于解密 Stripchat 加密的 HLS 分片文件名；支持配置同步 URL 自动拉取最新密钥
- **可视化后处理流水线**：DAG 节点编辑器，支持分叉与合并，内置模块：
    - **segment_merge** — 将录制分片目录（fMP4 .m4s）合并为单一视频文件（流水线首节点）
    - **contact_sheet** — 生成带时间戳的缩略图预览图
    - **filter_short** — 删除低于最短时长的录制文件
    - **notify_discord** — 通过 Discord Webhook 发送录制信息和封面图
    - **notify_telegram** — 通过 MTProto 发送录制信息、封面图和视频（支持超过 2 GB 的大文件，支持 HTTP/SOCKS5 代理）
    - **cleanup** — 后处理完成后清理录制相关文件及临时缓存
- **社区模块市场**：在 Web UI 中浏览、安装、更新社区贡献的后处理模块，支持配置下载代理和加速镜像
- **关于页面 / 自动更新**：检查 GitHub Releases 新版本；非 Docker 环境支持一键下载并自动替换程序文件；Docker 环境提示手动更新镜像；更新日志以 Markdown 格式渲染并可展开阅读，附贡献者列表与仓库 / Telegram 频道 / FFmpeg 构建源链接
- **服务状态页**：持续探测各依赖服务（StripChat API、StripChat 浏览页、camgirlfinder、GitHub、各 CDN 域）的连通性与延时，ECG 式滚动曲线展示；探测走与生产相同的通道（代理 / ECH / 镜像按当前设置生效），直连与 ECH 双腿并列对比，提供商徽标按响应头自动识别
- **资源占用页**：按进程树（主程序 / 模块 / ffmpeg / 其他，含模块唤起的子进程）实时统计 CPU、内存、PID、磁盘读写速率与 GPU 利用率及显存（Windows PDH / Linux fdinfo 采样），附系统网络收发卡与 CPU/内存/磁盘/网络/GPU 趋势图
- **应用内通知面板**：主播改名与失效提醒、FFmpeg 安装/更新里程碑、版本更新提醒等事件入库展示
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
        # image: ghcr.io/chantrail/stripchat-recorder:latest  # GHCR 镜像，与 Docker Hub 同步发布，可按网络环境选用
        container_name: stripchat-recorder
        restart: unless-stopped
        environment:
            - TZ=Asia/Shanghai
            # - LANGUAGE=zh-CN  # 设置界面语言，支持 zh-CN 或 en-US
            # - PORT=3030        # 设置服务端口（默认 3030）
        ports:
            - "${PORT:-3030}:${PORT:-3030}"
        volumes:
            - ./data/logs:/app/stripchat-recorder/logs # 运行日志
            - ./data/fragment:/app/stripchat-recorder/fragment # 录制中的分片（即分片流输出目录）
            - ./data/recordings:/app/stripchat-recorder/recordings # segment_merge 等模块的输出视频
            - ./data/modules:/app/stripchat-recorder/modules # 后处理模块可执行文件
            - ./data/meta:/app/stripchat-recorder/meta # 录制元数据
            - ./data/config:/app/stripchat-recorder/config # 配置文件（含 settings.json）
            - ./data/tools:/app/stripchat-recorder/tools # FFmpeg（程序自动下载或自备构建）
```

```bash
docker compose up -d
```

启动后在浏览器中打开 `http://localhost:3030`。首次访问时会进入设置向导，引导你完成语言选择、录制目录设置、网络代理配置、FFmpeg 来源检查和管理员密码设置。

Docker 镜像默认以 Server 模式运行（端口 3030），配置写入挂载的 `config/settings.json`。镜像为多架构构建（`linux/amd64`、`linux/arm64`），x86 与 ARM64 主机均可直接拉取运行。同一 tag 同时发布到 **Docker Hub**（`chantrail/stripchat-recorder`）与 **GHCR**（`ghcr.io/chantrail/stripchat-recorder`），内容一致，可按网络环境任选其一。**镜像不内置 FFmpeg**：首次启动由设置向导就地自动下载到挂载的 `tools` 目录（需能访问 GitHub Releases），也可预先把自备构建放入 `tools` 目录——详见下文「硬件要求与 `-legacy` 构建轨」。

### docker run

```bash
docker run -d \
  --name stripchat-recorder \
  --restart unless-stopped \
  -e TZ=Asia/Shanghai \
  -e LANGUAGE=zh-CN \
  -e PORT=3030 \
  -p 3030:3030 \
  -v ./data/logs:/app/stripchat-recorder/logs \
  -v ./data/fragment:/app/stripchat-recorder/fragment \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/meta:/app/stripchat-recorder/meta \
  -v ./data/config:/app/stripchat-recorder/config \
  -v ./data/tools:/app/stripchat-recorder/tools \
  chantrail/stripchat-recorder:latest
# 使用 GHCR：将镜像名替换为 ghcr.io/chantrail/stripchat-recorder:latest
```

---

## 主要设置项

在 Web UI 的「设置」页面可配置以下选项：

| 设置项                       | 说明                                                                                                                                                                                    |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 分片流输出目录               | 录制产生的分片（fMP4 .m4s）存放路径                                                                                                                                                     |
| 后处理最大并发数             | 同时运行的后处理任务数；`0` 表示自动（= CPU 逻辑核心数）；手动设置时上限同为 CPU 逻辑核心数，可主动设置低于自动值以压低天花板。实际并发会随系统负载自动下调，上限只是天花板             |
| 轮询间隔（秒）               | 检查主播是否上线的基础间隔，范围 10–300；有历史在线规律数据的主播在低活跃时段会自适应拉长（上限 200 秒）以节省流量                                                                      |
| 首选录制分辨率               | 目标录制画质（0 = 原始/最高画质），可设置画质不可用时的回退方向                                                                                                                         |
| 首选视频编码                 | 获取播放列表时附带 `preferredVideoCodec` 参数（H.264 / H.265 / AV1，不选则由服务器决定）；所选编码不可用时按 AV1→H.265→H.264 向下回退；H.265 / AV1 体积更小，但需播放器与后处理工具支持 |
| 录制文件时长（秒）           | 每个分片文件的最长时长，`0` 表示不限制；可用于按时段切割长播录制                                                                                                                        |
| 后处理临时目录最大占用（GB） | 后处理模块运行时产生的临时文件上限，超出后自动删除最旧的文件；`0` 表示不限制，默认 50 GB                                                                                                |
| FFmpeg 首选项                | 选择使用系统 PATH 中的 FFmpeg 还是程序目录 `tools` 文件夹中的 FFmpeg；未设置时启动后弹出选择向导                                                                                        |
| 空闲时自动更新 FFmpeg        | 定期检查并自动更新 `tools` 文件夹中由程序下载的 FFmpeg（无录制和后处理任务时执行），默认开启                                                                                            |
| 检查 beta 版本更新           | 是否同时检查预发布（beta）版本；beta 版本构建自动强制开启                                                                                                                               |
| 自定义播放器（Desktop）      | 播放转发流、录制文件与媒体文件时唤起的播放器程序路径；留空自动探测（转发流探测 mpv/VLC/PotPlayer 等，录制文件走系统默认关联）                                                           |

### 网络代理与镜像站

在设置页的「网络」中可分别配置：

Stripchat 镜像站项目：<https://github.com/ChanTrail/StripchatMirror>

1. **API 代理**：用于访问 Stripchat API；若同时填写镜像站，则通过该代理访问镜像站。
2. **Stripchat 镜像站**：用于替换请求中的 `stripchat.com` 域名，协议可选 http/https。
3. **浏览页代理**：StripChat 浏览页列表/快照请求专用，与 API 代理相互独立，不影响录制与状态轮询。
4. **浏览页镜像站**：StripChat 浏览页专用镜像站，与上方镜像站相互独立；同时填写浏览页代理时经该代理访问。
5. **CDN 代理**：用于下载直播分片流，可与 API 代理分开设置。
6. **社区模块代理**：用于下载社区模块文件，可独立配置。
7. **社区模块加速镜像**：在 GitHub 地址前加前缀（如 `https://ghproxy.com`），加速模块下载；同时填写社区代理时经该代理访问加速站。
8. **camgirlfinder 代理**：后端获取主播历史在线规律（schedule）时使用；主播查找页的人脸/名字搜索为纯前端功能，不经此代理。
9. **ECH（加密 ClientHello）**：开启后 StripChat API 请求的 TLS ClientHello 被加密以隐藏真实 SNI，可能改善 API 返回 403 的情况；不可路由或解析失败时自动回退普通连接。CDN 分片流量直取优先，仅在全部直连失败后的兜底竞速中带 ECH 腿。默认关闭。

### 管理员认证

首次访问时设置向导会引导创建管理员密码（至少 6 位，需包含字母、数字和特殊字符）。之后每次访问需要登录，Token 绑定登录 IP，有效期 8 小时，每次请求自动续期，最多支持 5 个并发登录会话。

在设置页的「安全」分区可以修改密码。

### Mouflon HLS 解密密钥

Stripchat 对 HLS 分片文件名进行了加密（Mouflon 系统）。若录制时遇到无法下载分片的情况，需在设置页的「Mouflon 解密密钥」中填入对应的 `pkey → pdkey` 密钥对；没有 pdkey 时也可以填 `keystream`（十六进制，至少 16 字节），同一 `pkey` 两者都有时优先使用 pdkey。密钥可从社区渠道获取。

也可以配置「同步地址」和可选的「同步令牌」，从指定 URL 自动拉取最新密钥。

### 转发流（HLS Relay）

在 Server 模式下，无需将主播添加到录制列表，直接用播放器打开以下地址即可播放直播；首次访问时自动连接上游，支持多个客户端同时连接同一转发流。在 Web UI 的「转发流」页面可查看所有活跃会话的状态、连接数和运行时长。

```
http://localhost:3030/stream/{modelname}?resolution=720&codec=h264&preference=higher
```

查询参数可临时覆盖流选择（缺省字段回退程序设置；参数不同的新请求会重建会话以应用新参数）：

- `resolution`：目标分辨率高度（像素，`0` = 原始画质）
- `codec`：首选视频编码（`h264` / `h265` / `av1`，大小写不敏感）
- `preference`：目标分辨率不可用时的回退方向（`higher` = 优先向上）

---

## 后处理模块

后处理流水线是一个 **DAG（有向无环图）**，在可视化编辑器中拖拽连线即可构建处理链路，支持分叉与合并。

### 内置模块

| 模块              | 说明                                                                 |
| ----------------- | -------------------------------------------------------------------- |
| `segment_merge`   | 将录制分片目录合并为单一视频文件，**官方流水线的首节点**             |
| `contact_sheet`   | 按配置间隔截帧并拼合为预览图                                         |
| `filter_short`    | 删除低于最短时长的录制文件                                           |
| `notify_discord`  | 通过 Discord Webhook 发送录制信息和封面图                            |
| `notify_telegram` | 通过 MTProto 向 Telegram 发送录制信息、封面图和视频                  |
| `cleanup`         | 清理录制相关文件及其他模块产生的临时缓存，通常作为流水线最后一个节点 |

### 社区模块市场

在 Web UI 的「模块社区」页面可以浏览、安装和更新由社区贡献的第三方后处理模块。安装前会展示免责声明；支持通过设置页配置下载代理和加速镜像。

自定义模块放入 `modules` 数据卷目录后会被自动发现，且不会在容器重启时被覆盖。详见[后处理模块开发文档](docs/module-development.md)和[社区模块文档](docs/community-registry.md)。

> **文件名格式：** 主程序只加载文件名符合 `{name}-{platform}-{version}` 格式的可执行文件。

### Desktop 端模块安装路径

Desktop 端安装包（NSIS、deb、dmg）不把模块放在程序安装目录下。请将下载的 `modules-{platform}.zip` 解压到每用户数据目录的 `modules` 子目录：

| 平台    | 路径                                                                              |
| ------- | --------------------------------------------------------------------------------- |
| Windows | `%APPDATA%\com.chantrail.stripchat-recorder\modules\`                             |
| macOS   | `~/Library/Application Support/com.chantrail.stripchat-recorder/modules/`         |
| Linux   | `~/.local/share/com.chantrail.stripchat-recorder/modules/`（或 `$XDG_DATA_HOME`） |

目录不存在时可手动创建；应用启动后会自动监控该目录，新增/删除模块无需重启。

**便携版（Windows `*-portable.zip`、Linux AppImage）**：数据目录跟随程序本体——`config/`、`modules/`、`tools/` 等都在可执行文件（Windows 为 exe 同目录、AppImage 为 .AppImage 文件所在目录）旁边，模块直接放进旁边的 `modules/` 即可。Windows 便携 zip 已内含模块。首次运行时若旧安装版的每用户数据目录里已有 `config/`、`locale/`、`modules/`，会自动迁移过来（仅当便携目录中对应目录不存在）。

---

## FFmpeg 自动管理

程序依赖 ffmpeg 与 ffprobe，按以下顺序自动管理，通常无需手动安装：

- **查找顺序**：启动时先检查 `tools` 文件夹（Server 为程序目录下；Desktop 为与 `modules` 相同的数据根目录下——安装版是每用户数据目录，便携版/AppImage 是程序本体所在目录），再在系统 PATH 中查找。`tools` 中的副本优先于 PATH。
- **自动下载**：两处都找不到时，自动从 [FFmpeg-Builds-Full](https://github.com/ChanTrail/FFmpeg-Builds-Full) 下载预编译静态构建（约 130–210 MB，Windows 为 7z 固实压缩包，Linux/macOS 为 tar.xz）到 `tools` 文件夹，下载进度以 toast 提示（支持 Windows x86_64/ARM64、Linux x86_64/ARM64、macOS ARM64）。
- **FFmpeg 首选项**：尚未选择来源时（如旧版本升级后 `ffmpeg_preference` 为空），启动后会弹出选择向导，列出各位置检测结果供指定使用系统 PATH 还是 `tools` 文件夹中的 FFmpeg，也可在设置页随时修改；选择 PATH 来源时自动更新功能停用。
- **空闲自动更新**：下载的副本会在没有录制和后处理任务运行时自动更新到最新版本（设置项「空闲时自动更新 FFmpeg」，默认开启；程序启动即检查一次）。PATH 中的 FFmpeg，以及手动放入 `tools`、没有 `ffmpeg-build.json` 标记文件的 FFmpeg 都不会被替换。检测到新版、开始安装、完成/失败等里程碑会写入应用内通知面板。
- **Docker 镜像**不内置 FFmpeg：首次启动由设置向导引导就地自动下载到 `tools`（镜像内是 `VOLUME`，下载副本随卷持久并带 `ffmpeg-build.json` 标记，容器内同样参与空闲自动更新），构建轨与形态在向导下载步骤中可选；或指定系统 PATH 中的 FFmpeg；也可预先把自备构建放进挂载的 `tools` 目录，程序即按 tools 优先使用。

### 硬件要求与 `-legacy` 构建轨

FFmpeg-Builds-Full 分**平台轨**（linux 提供默认轨与 `-legacy` 兼容轨）与**变体形态**（静态 / `-shared` / `-debug` / `-shared-debug`）。两条轨启用库与功能完全一致，差异只在 CPU 基线世代：

| 构建轨                                               | x86_64                                                                               | arm64                           |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------ | ------------------------------- |
| 默认（`linux-x86_64` / `linux-arm64`）               | **x86-64-v3+**（AVX2/BMI2/FMA3，≈Haswell/2013 起）                                   | 任意 ARMv8                      |
| `-legacy`（`linux-x86_64-legacy` / `-arm64-legacy`） | SSE2 基线，任意 x86-64 CPU（AVX2+ SIMD 走 cpuid 运行时分发，老 CPU 自动回落 C 路径） | ARMv8.0 基线（Cortex-A53 实测） |

- **在设置向导中选择构建**：设置向导与「FFmpeg 首选项」弹窗的下载步骤提供「构建轨 × 形态」选择（轨：默认 / `-legacy`；形态：静态 / `-shared` / `-debug` / `-shared-debug`）。x86_64 宿主的 CPU 若早于 Haswell（无 AVX2），在向导中选择 `-legacy` 轨即可——无需手动放置文件。已安装的副本经 `ffmpeg-build.json` 标记记录所选轨与形态，空闲自动更新始终下载同一构建。
- **也可手动放置**：把对应构建解压进 `tools/`（Docker 即挂载的 `./data/tools`），不写 `ffmpeg-build.json` 标记 → 被视作手动放置，空闲自动更新不会替换它；或改用系统 PATH 中的 FFmpeg（选择 PATH 来源后自动更新自动停用）。
- **Docker**：镜像不内置 FFmpeg，首次启动由设置向导引导下载——在向导里直接选 `-legacy` 轨即可，容器内无需手动替换文件。
- arm64 宿主机（树莓派、ARM VPS 等）两条轨均可运行，无需特别处理；Windows 与 macOS 只有默认轨，同样无需关心本项。

---

## 从源码构建

**前置依赖：** Rust、Node.js (LTS)、ffmpeg（可选：缺失时程序启动后会自动下载到程序目录的 `tools` 文件夹）

### 首次启动配置

直接运行二进制文件时，若 `config/settings.json` 中尚未完成向导配置，会自动进入 Web 设置向导：

1. 选择界面语言
2. 设置录制输出目录
3. 配置网络代理（可选）
4. FFmpeg 检查（选择来源位置；`tools` 缺失时可就地触发下载，构建轨与形态可选，进度内联显示）
5. 设置管理员密码

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
- **容器：** Debian（FFmpeg 由程序在容器内按需下载管理，非镜像内置）

---

## 开源许可证

本项目基于 [GNU 通用公共许可证 v3.0](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html) 发布。

---

## 免责声明

本项目仅用于技术研究与学习交流。使用者需自行承担部署、运维与合规风险。
