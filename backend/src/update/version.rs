//! 版本号判定与比较：beta 预发布识别 + 数字三段式语义化比较。
//! Version predicates and comparison: beta pre-release check + numeric semver comparison.

use super::APP_VERSION;

/// 判断当前版本是否为预发布版（版本号以 `-beta` 结尾，如 `0.4.0-beta`）。
/// Returns true if the current version string ends with `-beta` (e.g. `0.4.0-beta`).
pub fn is_beta_version() -> bool {
    APP_VERSION.ends_with("-beta")
}

/// 语义化版本比较：`latest` > `current` 时返回 true。
///
/// 剥除 `-beta` 等预发布后缀后只比较 major.minor.patch 数字部分，
/// 从左到右逐段比较，遇到差异立即返回结果。
///
/// Compares only the numeric major.minor.patch part after stripping any
/// pre-release suffix. Segments are compared left-to-right; returns as soon
/// as a difference is found.
pub fn semver_gt(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.splitn(3, '.');
        let a = it.next()?.parse::<u64>().ok()?;
        let b = it.next()?.parse::<u64>().ok()?;
        let patch_str = it.next().unwrap_or("0");
        let c = patch_str
            .split('-')
            .next()
            .unwrap_or("0")
            .parse::<u64>()
            .ok()?;
        Some((a, b, c))
    }
    match (parse(latest), parse(current)) {
        (Some((la, lb, lc)), Some((ca, cb, cc))) => (la, lb, lc) > (ca, cb, cc),
        _ => latest != current,
    }
}
