# BoringSSL 工具链回避：Windows ARM64（aarch64-pc-windows-msvc）
# BoringSSL toolchain workaround for Windows ARM64 (aarch64-pc-windows-msvc).
#
# 背景 / Background:
#   btls-sys 0.5.6 的 build.rs 在原生构建（host == target）时提前返回，
#   跳过了它为「Windows 交叉编译」准备的 OPENSSL_NO_ASM 定义。BoringSSL
#   自带 CMakeLists 在 ARM64 Windows 上不满足 NASM 分支（CMAKE_SYSTEM_PROCESSOR
#   仅匹配 AMD64|x86_64 等），转而启用 OPENSSL_ASM，把 gen/ 下全平台 GAS 语法
#   .S 汇编源（含 aes-gcm-avx2-x86_64-apple.S 等）都加进工程。MSVC 的
#   armasm64 无法汇编 GAS 语法，对应 .obj 产不出来，链接阶段报
#   LNK1181（cannot open input file ... .obj）。
#
#   btls-sys 0.5.6's build.rs returns early when host == target, so the
#   OPENSSL_NO_ASM define it applies for Windows *cross* builds never fires
#   on native ARM64 Windows. BoringSSL's own CMakeLists then takes the
#   OPENSSL_ASM path (CMAKE_SYSTEM_PROCESSOR only matches AMD64|x86_64 for
#   the NASM branch) and adds every pregenerated GAS-syntax .S source —
#   including x86_64/apple files — to the project. MSVC's armasm64 cannot
#   assemble GAS syntax, so the expected .obj files are never produced and
#   linking fails with LNK1181.
#
#   btls-sys 与 cmake-rs 都会读取 CMAKE_TOOLCHAIN_FILE 环境变量：前者一旦
#   检测到该变量就把 cmake 配置权完全交给调用方，后者自动把它透传为
#   -DCMAKE_TOOLCHAIN_FILE。CI 在 windows-11-arm 上通过该变量注入本文件。
#
#   Both btls-sys and cmake-rs honour the CMAKE_TOOLCHAIN_FILE env var:
#   btls-sys hands full cmake configuration over to the caller when it is
#   set, and cmake-rs forwards it as -DCMAKE_TOOLCHAIN_FILE. CI injects
#   this file on the windows-11-arm runner.
#
# 用法 / Usage:
#   set CMAKE_TOOLCHAIN_FILE=<repo>/cmake/boringssl-win-arm64.cmake
#   （本地 Windows ARM64 机器上构建同样需要设置 / needed for local builds
#   on Windows ARM64 machines as well）

# 关闭 BoringSSL 汇编路径：.S 源不再进入工程，全部走 C 实现。
# Disable BoringSSL assembly: .S sources stay out of the project and the
# C implementations are used instead.
set(OPENSSL_NO_ASM YES CACHE BOOL "Disable BoringSSL asm on Windows ARM64 / 在 Windows ARM64 上禁用 BoringSSL 汇编" FORCE)

# btls-sys 见到 toolchain 文件后提前返回，不再定义 CMAKE_MSVC_RUNTIME_LIBRARY；
# 这里补上与它原本一致的 /MD（动态 CRT），匹配非 crt-static 的 Rust 目标。
# Once a toolchain file is set, btls-sys returns early and no longer defines
# CMAKE_MSVC_RUNTIME_LIBRARY; restore the /MD (dynamic CRT) it would have
# used, matching Rust targets without the crt-static feature.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreadedDLL" CACHE STRING "MSVC runtime library / MSVC 运行时库")
