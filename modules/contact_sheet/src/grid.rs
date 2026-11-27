//! 网格布局计算 / Grid Layout Calculation

/// 根据总帧数和用户偏好计算最优列数。
///
/// Compute the optimal column count for the grid image.
///
/// - `frame_count`: 总帧数 / total frame count
/// - `forced_cols`: 用户指定值，0 = 自动 / user-specified value, 0 = auto
pub fn compute_cols(frame_count: u32, forced_cols: u32) -> u32 {
    if forced_cols > 0 {
        return forced_cols;
    }
    // sqrt × 1.33 使网格略宽于高 / sqrt × 1.33 makes the grid slightly wider than tall
    (((frame_count as f64).sqrt() * 1.33).ceil() as u32).max(1)
}

/// 根据帧数、列数和用户强制行数计算最终行数。
///
/// Compute the final row count from frame count, column count, and optional forced rows.
///
/// - `frame_count`: 总帧数 / total frame count
/// - `cols`: 列数 / column count
/// - `forced_rows`: 用户指定值，0 = 自动 / user-specified value, 0 = auto
pub fn compute_rows(frame_count: u32, cols: u32, forced_rows: u32) -> u32 {
    if forced_rows > 0 {
        return forced_rows;
    }
    // 向上取整，保证所有帧都能放入网格 / ceiling division to fit all frames
    frame_count.div_ceil(cols)
}
