# 社区模块发布指南

[简体中文](community-registry.md) | [English](community-registry.en.md)

---

## 架构概述

社区模块系统采用**两级 registry** 设计：

```
中央索引（本项目维护）              模块维护者仓库
────────────────────               ────────────────────────────────
registry.json                      registry.json（由 CI 自动维护）
[                                  {
  { "id": "00000001",                "id": "my_module",
    "repo": "github.com/..." }  →    "name": "My Module",
]                                    "latestVersion": "1.2.0",
                                     "downloads": { ... },
                                     "sha256": { ... }
                                   }
```

- **中央索引**：只存序号和仓库地址，向中央提 PR **只在第一次注册时需要**。
- **模块元数据**：由 CI 自动写入维护者仓库的 `registry.json`，版本升级时只需改 `Cargo.toml` 版本号并 push，无需手动操作任何文件。

---

## 第一步：创建模块仓库

1. 在 GitHub 创建**公开仓库**，建议命名 `stripchat-pp-<your-module-id>`
2. 复制 [module-template/](https://github.com/ChanTrail/StripchatRecorderCommunity/tree/master/module-template) 的内容到你的仓库
3. 修改以下文件（只需改一次）：
   - `Cargo.toml`：改 `name`（与模块 ID 保持一致）和 `version`
   - `src/main.rs`：将 `my_module` 替换为你的模块 ID，实现 `run()` 函数
   - `registry.json`：填写 `id`、`name`、`description`、`author`、`tags`（其余字段由 CI 自动填写）
   - 两个 workflow 文件顶部的 `MODULE_NAME`

参考[模块开发文档](module-development.md)了解完整的 stdin/stdout 协议。

---

## 第二步：配置 RELEASE_PAT

在仓库 **Settings → Secrets → Actions** 中添加名为 `RELEASE_PAT` 的 secret：

1. GitHub 右上角头像 → **Settings** → **Developer settings** → **Personal access tokens** → **Tokens (classic)**
2. 点 **Generate new token (classic)**，勾选 **repo** 权限，点 **Generate token**
3. 复制生成的 token（`ghp_xxx...`，只显示一次）
4. 回到模块仓库，进入 **Settings → Secrets → Actions**，点 **New repository secret**
5. **Name** 填 `RELEASE_PAT`，**Secret** 粘贴刚才复制的 token，点 **Add secret**

> **为什么需要 PAT？**  
> GitHub Actions 默认的 `GITHUB_TOKEN` 推送的 tag 不会触发其他 workflow（安全限制）。
> 用 PAT 推 tag 才能让 `release.yml` 被触发。

---

## 第三步：初始化 registry.json

模板中的 `registry.json` 只需填写三个字段，**其余全部由 Release workflow 自动生成**：

```json
{
  "description": "你的模块功能描述",
  "tags": ["notify", "upload"],
  "license": "MIT"
}
```

Release workflow 会自动推导并写入：

| 字段            | 来源 |
|-----------------|------|
| `id`            | `Cargo.toml` 的 `name` |
| `name`          | `id` 的 Title Case 形式（`my_module` → `My Module`） |
| `author`        | 仓库 owner（`owner/repo` 中的 `owner`） |
| `latestVersion` | `Cargo.toml` 的 `version` |
| `downloads`     | 根据 tag 和平台名称自动构造 |
| `sha256`        | 从构建产物的 `.sha256` 文件读取 |

### 支持的平台标识符

| 标识符           | 说明                      |
|------------------|---------------------------|
| `windows-x86_64` | Windows 64 位             |
| `linux-x86_64`   | Linux x86_64              |
| `linux-aarch64`  | Linux ARM64               |
| `darwin-x86_64`  | macOS Intel               |
| `darwin-aarch64` | macOS Apple Silicon       |

---

## 完整发布流程

配置完成后，**发布新版本只需两步**：

```bash
# 1. 修改 Cargo.toml 中的 version 字段
# 2. push 到 main
git add Cargo.toml
git commit -m "chore: bump version to 1.0.0"
git push origin master
```

之后全部自动完成：

```
push main
   ↓
ci.yml：check / clippy / test 通过
   ↓
ci.yml：读取 Cargo.toml 版本号 → 推送 tag（通过 RELEASE_PAT）
   ↓
release.yml：多平台编译 + sha256 计算
   ↓
release.yml：用 jq 更新 registry.json（latestVersion / downloads / sha256）
           并用 RELEASE_PAT commit 回 main
   ↓
release.yml：创建 GitHub Release 并上传所有产物
   ↓
StripchatRecorder 下次刷新时自动获取到新版本
```

---

## 第四步：注册到中央索引（仅需一次）

1. Fork [StripchatRecorderCommunity](https://github.com/ChanTrail/StripchatRecorderCommunity)
2. 在 `registry.json` 末尾追加一行：

```json
{ "id": "00000NNN", "repo": "https://github.com/your-username/stripchat-pp-your-module" }
```

`id` 取当前最大序号 +1，补齐 8 位（例如当前最大为 `00000003`，则新增 `00000004`）。

3. 提交 PR，标题：`Add module: {your_module_id}`

---

## 升级版本（后续完全自治）

修改 `Cargo.toml` 中的 `version` 字段并 push，**不需要任何其他操作**。
