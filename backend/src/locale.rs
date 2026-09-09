pub mod manager;

/// 查询后端日志翻译并进行参数插值。
///
/// Look up a backend log translation and interpolate parameters.
///
/// # 用法 / Usage
/// ```rust
/// // 无参数 / No params
/// tl!("recorder.stopSignal")
///
/// // 有参数（key = value，value 会被 Display 格式化）/ With params
/// tl!("recorder.started", username = username, dir = session_dir.display())
/// ```
///
/// key 对应 `locale/log/<lang>.json` 中（不含 `log.` 前缀）的路径，
/// 例如 `"recorder.started"` 对应 JSON 的 `recorder.started` 字段。
#[macro_export]
macro_rules! tl {
    // 无参数 / No params
    ($key:literal) => {
        $crate::locale::manager::tl_log($key, &[])
    };
    // 有参数：tl!("key", name1 = expr1, name2 = expr2, ...)
    ($key:literal, $( $name:ident = $val:expr ),+ $(,)?) => {{
        let params: &[(&str, String)] = &[$( (stringify!($name), format!("{}", $val)) ),+];
        let params_ref: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        $crate::locale::manager::tl_log($key, &params_ref)
    }};
}
