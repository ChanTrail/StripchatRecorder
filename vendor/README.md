# Vendored crates / 内嵌依赖

## wreq 0.16.4

本目录是 `wreq` 0.16.4 的 vendored 副本，带一处私有补丁：

This directory vendors `wreq` 0.16.4 with one private patch:

- **`TlsOptions::ech_config_list`**（`src/tls.rs` + `src/tls/conn.rs`）：
  新增 `ECHConfigList` 注入点，在 `setup_ssl`/`setup_ssl2` 握手前调用
  `SSL_set1_ech_config_list`——wreq 公开 API 只有 `enable_ech_grease`，
  无法发起真实 ECH。
- **`TlsOptions::ech_config_list`** (`src/tls.rs` + `src/tls/conn.rs`):
  adds an `ECHConfigList` injection point, calling
  `SSL_set1_ech_config_list` before the handshake in
  `setup_ssl`/`setup_ssl2` — upstream only exposes `enable_ech_grease`,
  which cannot offer real ECH.

升级 wreq 时需重新移植该补丁（见 `git log -- vendor/wreq`）。上游若
合并等效功能（issue/PR 待提），可切回 crates.io 版本。

When upgrading wreq, re-apply this patch (see `git log -- vendor/wreq`).
If upstream merges an equivalent feature (issue/PR TBD), switch back to
the crates.io release.
