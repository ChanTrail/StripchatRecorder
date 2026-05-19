/**
 * 录制文件相关类型定义 / Recording File Type Definitions
 */

/** 后处理模块执行结果 / Post-processing module execution result */
export interface PpModuleResult {
	/** 模块 ID / Module ID */
	module_id: string;
	/** 是否成功 / Whether succeeded */
	success: boolean;
	/** 结果消息 / Result message */
	message: string;
}

/**
 * 录制文件状态 / Recording file status
 *
 * - `recording`       — 正在录制
 * - `merging_waiting` — 等待合并（排队中）
 * - `merging`         — 正在合并 TS 分片
 * - `pp_waiting`      — 等待后处理（排队中）
 * - `pp_running`      — 后处理执行中
 * - `pp_error`        — 后处理失败
 * - `finish`          — 全部完成
 */
export type RecordingStatus =
	| "recording"
	| "merging_waiting"
	| "merging"
	| "pp_waiting"
	| "pp_running"
	| "pp_error"
	| "finish";

/** 录制文件元数据 / Recording file metadata */
export interface RecordingFile {
	/** 文件名（含扩展名）/ Filename (with extension) */
	name: string;
	/** 文件完整路径 / Full file path */
	path: string;
	/** 文件大小（字节）/ File size (bytes) */
	size_bytes: number;
	/** 录制开始时间（ISO 字符串）/ Recording start time (ISO string) */
	started_at: string;
	/** 是否正在录制 / Whether currently recording */
	is_recording: boolean;
	/** 已录制时长（秒），录制中时实时更新 / Recorded duration (seconds), updated in real-time while recording */
	record_duration_secs: number | null;
	/** 视频实际时长（秒），由 ffprobe 获取并写入 meta / Actual video duration (seconds), obtained via ffprobe and stored in meta */
	video_duration_secs: number | null;
	/** 当前处理状态（来自 meta 文件）/ Current processing status (from meta file) */
	status?: RecordingStatus | null;
	/** 各模块后处理结果（来自 meta 文件）/ Per-module post-processing results (from meta file) */
	pp_results?: PpModuleResult[] | null;
	/** 模块输出路径（来自 meta 文件）/ Module output paths (from meta file) */
	module_outputs?: Record<string, string> | null;
}
