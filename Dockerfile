FROM debian:latest AS builder

LABEL maintainer="chantrail@chantrail.com" \
      version="0.5.3" \
      description="Stripchat Recorder Docker builder"

RUN apt-get update && apt-get install -y \
    curl \
    pkg-config \
    build-essential \
    libssl-dev \
    # wreq 的 BoringSSL(btls-sys) 构建依赖：cmake 驱动、nasm 汇编器、
    # perl 供 perlasm 代码生成；git 供 build script 用 git init/apply
    # 打 bundled boringssl 补丁；libclang 供 bindgen 运行时生成绑定。/
    # Build deps for wreq's BoringSSL (btls-sys): cmake driver, nasm
    # assembler, perl for perlasm codegen; git for the build script's
    # git init/apply of bundled boringssl patches; libclang for bindgen.
    cmake \
    nasm \
    perl \
    git \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - && \
    apt-get install -y nodejs && \
    rm -rf /var/lib/apt/lists/*

# pnpm 版本与根 package.json 的 packageManager 保持一致；借 Node 自带的 npm
# 引导安装（Node 25+ 不再内置 corepack）/ pnpm pinned to the root
# package.json packageManager; bootstrapped via Node's bundled npm (Node 25+
# no longer ships corepack)
RUN npm install -g pnpm@12.23.0

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \
    . /root/.cargo/env

WORKDIR /app
COPY . /app

RUN . /root/.cargo/env && pnpm run build

# ── Runtime image ──────────────────────────────────────────────────────────────
FROM debian:latest

LABEL maintainer="chantrail@chantrail.com" \
      version="0.5.3" \
      description="Stripchat Recorder"

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /app/stripchat-recorder/logs \
             /app/stripchat-recorder/fragment \
             /app/stripchat-recorder/recordings \
             /app/stripchat-recorder/modules.default \
             /app/stripchat-recorder/modules \
             /app/stripchat-recorder/meta \
             /app/stripchat-recorder/config
WORKDIR /app

COPY --from=builder /app/build/stripchat-recorder /app/stripchat-recorder/
COPY --from=builder /app/build/modules/ /app/stripchat-recorder/modules.default/

# 镜像不内置 FFmpeg：首次启动由设置向导引导选择来源（就地自动下载 /
# 系统 PATH），或把自备构建放进挂载的 tools/ 目录——tools 在 VOLUME
# 中，容器内下载与用户自放的副本都能跨重建持久；无 ffmpeg-build.json
# 标记的自放副本不会被空闲自动更新替换。不前置到 PATH：程序按首选项
# 解析 tools 二进制，模块子进程 PATH 由程序注入 tools 目录。
# FFmpeg is not baked into the image: the setup wizard guides the source
# choice on first start (in-place auto-download / system PATH), or drop a
# self-provided build into the mounted tools/ dir — tools is a VOLUME, so
# both downloaded and user-placed copies survive container rebuilds, and an
# unmarked self-placed copy is never replaced by idle auto-update. No PATH
# prepend: the program resolves tools binaries per its preference and
# injects the tools dir into module child processes' PATH itself.
RUN mkdir -p /app/stripchat-recorder/tools

RUN chmod +x /app/stripchat-recorder/stripchat-recorder

RUN printf '%s\n' \
    '#!/bin/sh' \
    'set -eu' \
    '' \
    'cp -an /app/stripchat-recorder/modules.default/. /app/stripchat-recorder/modules/' \
    '' \
    '# LANGUAGE / PORT env vars are applied to config/settings.json by the program itself' \
    '# at startup (it also creates the file on first run)' \
    'exec /app/stripchat-recorder/stripchat-recorder "$@"' \
    > /entrypoint.sh && chmod +x /entrypoint.sh

VOLUME ["/app/stripchat-recorder/logs", "/app/stripchat-recorder/fragment", "/app/stripchat-recorder/recordings", "/app/stripchat-recorder/modules", "/app/stripchat-recorder/meta", "/app/stripchat-recorder/config", "/app/stripchat-recorder/tools"]

EXPOSE ${PORT:-3030}

ENTRYPOINT ["/entrypoint.sh"]
