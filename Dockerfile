FROM debian:latest AS builder

LABEL maintainer="chantrail@chantrail.com" \
      version="0.4.9-beta" \
      description="Stripchat Recorder Docker builder"

RUN apt-get update && apt-get install -y \
    curl \
    pkg-config \
    build-essential \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - && \
    apt-get install -y nodejs && \
    rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \
    . /root/.cargo/env

WORKDIR /app
COPY . /app

RUN . /root/.cargo/env && npm run build

# ── FFmpeg stage：自编译静态二进制（FFmpeg-Builds-Full latest release）──────────
# ── FFmpeg stage: self-built static binaries (FFmpeg-Builds-Full latest release) ──
FROM debian:latest AS ffmpeg

# TARGETARCH 由 buildx 自动注入（amd64 / arm64）；传统 docker build 无此值，
# 默认按 amd64 处理。FFMPEG_RELEASE_BASE 可覆盖为镜像前缀（ghproxy 风格）。
# TARGETARCH is injected by buildx (amd64 / arm64); a legacy docker build
# leaves it empty and falls back to amd64. FFMPEG_RELEASE_BASE can be
# overridden with a mirror prefix (ghproxy-style).
ARG TARGETARCH=amd64
ARG FFMPEG_RELEASE_BASE=https://github.com/ChanTrail/FFmpeg-Builds-Full/releases/download/latest

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# 下载压缩包 + 同名 .sha256 → sha256sum 校验 → 仅提取 */bin/ 下的
# ffmpeg / ffprobe / ffplay 到 /tools（与程序内 tools/ 目录约定一致；
# 静态构建无共享库依赖，提取后以 -version 冒烟验证可执行）
# Download the archive + same-named .sha256 → verify with sha256sum → extract
# only ffmpeg / ffprobe / ffplay under */bin/ into /tools (matching the
# program's tools/ convention; the static build has no shared-lib deps and is
# smoke-tested with -version after extraction)
RUN set -eux; \
    case "$TARGETARCH" in \
        amd64) plat="linux-x86_64" ;; \
        arm64) plat="linux-arm64" ;; \
        *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    asset="ffmpeg-latest-${plat}.tar.xz"; \
    mkdir -p /tmp/ffdl/ex /tools; \
    cd /tmp/ffdl; \
    curl -fSL --retry 3 -o "$asset" "${FFMPEG_RELEASE_BASE}/${asset}"; \
    curl -fSL --retry 3 -o "${asset}.sha256" "${FFMPEG_RELEASE_BASE}/${asset}.sha256"; \
    echo "$(cut -d' ' -f1 "${asset}.sha256")  ${asset}" | sha256sum -c -; \
    tar -xJf "$asset" -C ex; \
    for b in ffmpeg ffprobe ffplay; do \
        src="$(find ex -type f -path "*/bin/${b}" | head -n1)"; \
        [ -n "$src" ] || { echo "archive missing */bin/${b}" >&2; exit 1; }; \
        cp "$src" "/tools/${b}"; \
    done; \
    chmod 755 /tools/ffmpeg /tools/ffprobe /tools/ffplay; \
    /tools/ffmpeg -version | head -n1; \
    rm -rf /tmp/ffdl

# ── Runtime image ──────────────────────────────────────────────────────────────
FROM debian:latest

LABEL maintainer="chantrail@chantrail.com" \
      version="0.4.9-beta" \
      description="Stripchat Recorder"

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /app/stripchat-recorder/logs \
             /app/stripchat-recorder/ts_fragment \
             /app/stripchat-recorder/recordings \
             /app/stripchat-recorder/modules.default \
             /app/stripchat-recorder/modules \
             /app/stripchat-recorder/meta \
             /app/stripchat-recorder/config
WORKDIR /app

COPY --from=builder /app/build/stripchat-recorder /app/stripchat-recorder/
COPY --from=builder /app/build/modules/ /app/stripchat-recorder/modules.default/

# 自编译 FFmpeg 落入 tools/：程序默认优先使用 `<exe_dir>/tools/` 下的副本；
# 无 ffmpeg-build.json 标记即视为手动放置，自动更新不会替换镜像内副本。
# PATH 前置保证模块子进程与 docker exec 里裸 `ffmpeg` 命令可用。
# The self-built FFmpeg lands in tools/: the program prefers the copy under
# `<exe_dir>/tools/` by default; without an ffmpeg-build.json marker it counts
# as manually placed and auto-update never replaces the in-image copy.
# Prepending tools to PATH keeps bare `ffmpeg` usable for module child
# processes and docker exec.
COPY --from=ffmpeg /tools/ /app/stripchat-recorder/tools/
ENV PATH="/app/stripchat-recorder/tools:${PATH}"

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

VOLUME ["/app/stripchat-recorder/logs", "/app/stripchat-recorder/ts_fragment", "/app/stripchat-recorder/recordings", "/app/stripchat-recorder/modules", "/app/stripchat-recorder/meta", "/app/stripchat-recorder/config"]

EXPOSE ${PORT:-3030}

ENTRYPOINT ["/entrypoint.sh"]
