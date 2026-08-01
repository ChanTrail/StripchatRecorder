/**
 * 录制文件相关类型定义 / Recording File Type Definitions
 */

/** 后处理节点执行结果码 / Post-processing node execution result code */
export type PpExecCode = "ok" | "done" | "skipped" | "error" | "cancelled";

/** 后处理节点执行结果 / Post-processing node execution result */
export interface PpExecResult {
	code: PpExecCode;
	message?: string | null;
}

/** 后处理流水线单个节点的执行记录 / Execution record for a single post-processing pipeline node */
export interface PpExecutionEntry {
	/** 节点唯一 ID / Node unique ID */
	node_id: string;
	/** 模块 ID / Module ID */
	module_id: string;
	/** 节点开始执行时间（RFC 3339）/ Node execution start time (RFC 3339) */
	started_at: string;
	/** 节点完成时间，执行中为 null / Finish time, null while running */
	finished_at: string | null;
	/** 执行结果，执行中为 null / Execution result, null while running */
	result: PpExecResult | null;
	/** 输入路径列表 / Input path list */
	inputs: string[];
	/** 输出路径列表，执行中为 null / Output path list, null while running */
	outputs: string[] | null;
}

/**
 * 录制文件状态 / Recording file status
 *
 * - `recording`  — 正在录制
 * - `pp_waiting` — 等待后处理（含 ts_merge 合并，排队中）
 * - `pp_running` — 后处理执行中（含 ts_merge 合并）
 * - `pp_error`   — 后处理失败
 * - `finish`     — 全部完成
 *
 * 注：合并 TS 分片现由 ts_merge 后处理模块负责，不再有单独的 merging 状态。
 * Note: TS segment merging is now handled by the ts_merge module; no separate merging status.
 */
export type RecordingStatus =
	| "recording"
	| "pp_waiting"
	| "pp_running"
	| "pp_error"
	| "finish";

/** 正在执行的节点的模块内进度快照 / In-progress node's intra-module progress snapshot */
export interface PpNodeProgress {
	node_id: string;
	module_id: string;
	mod_done: number;
	mod_total: number;
	overall_done: number;
	overall_total: number;
}

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
	/** 视频分辨率（如 "1920x1080"），由 ffprobe 获取并写入 meta / Video resolution (e.g. "1920x1080"), obtained via ffprobe and stored in meta */
	video_resolution?: string | null;
	/** 当前处理状态（来自 meta 文件）/ Current processing status (from meta file) */
	status?: RecordingStatus | null;
	/** 后处理流水线各节点的执行记录（来自 meta 文件）/ Post-processing pipeline node execution records (from meta file) */
	pp_execution?: PpExecutionEntry[] | null;
	/** 累计成功下载的分片数（录制中实时更新）/ Total successfully downloaded segments (updated in real-time while recording) */
	segments_downloaded?: number | null;
	/** 累计下载失败的分片数（录制中实时更新）/ Total failed segment downloads (updated in real-time while recording) */
	segments_failed?: number | null;
	/** 当前正在执行节点的模块内进度（未在执行时为 null）/ Intra-module progress of the running node (null when idle) */
	pp_progress?: PpNodeProgress | null;
}
