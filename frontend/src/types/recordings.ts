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

/**
 * 后处理流水线单个节点的执行记录 / Execution record for a single post-processing pipeline node
 *
 * `inputs`/`outputs` 按端口分组：外层数组下标对应端口索引，内层数组是该端口承载的
 * 路径列表（大多数端口只有一个路径；MediaBundle 端口会拆分为多个字符串，如
 * `[视频路径, 图片路径]`）。两者始终存在，节点尚无输出时 `outputs` 为空数组。
 *
 * `inputs`/`outputs` are grouped by port: the outer array's index is the port index,
 * and the inner array holds the path(s) carried on that port (most ports have a single
 * path; a MediaBundle port is split into multiple strings, e.g. `[videoPath, imagePath]`).
 * Both are always present; `outputs` is an empty array before the node has produced output.
 */
export interface PpExecutionEntry {
	/** 模块 ID（普通节点的主标识）/ Module ID (primary identifier for regular nodes) */
	module_id: string;
	/** 节点实例 ID（仅可复用内置节点的多个实例需要）/ Node instance ID (only for reusable built-in multi-instances) */
	node_id?: string | null;
	started_at: string;
	finished_at: string | null;
	result: PpExecResult | null;
	inputs: string[][];
	outputs: string[][];
}

/** 辅助函数：返回执行记录的有效唯一标识 / Helper: effective identifier for an execution entry */
export function entryEffectiveId(entry: PpExecutionEntry): string {
	return entry.node_id ?? entry.module_id;
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
	module_id: string;
	/** 节点实例 ID（仅可复用内置节点多实例时需要）*/
	node_id?: string | null;
	mod_done: number;
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
	/**
	 * 模块输出路径（如 contact_sheet 生成的预览图），按 module_id 建立映射。
	 * 后端已验证：节点执行结果为 `"ok"` 且路径当前确实存在于磁盘上——前端应仅
	 * 依据此字段判断预览图按钮是否显示，不要自行从 pp_execution 推断路径
	 * （前端没有文件系统访问权限，无法验证路径是否真实存在）。
	 *
	 * Module output paths (e.g. contact_sheet's generated preview image), keyed by
	 * module_id. Backend-verified: the node's result is `"ok"` and the path currently
	 * exists on disk — the frontend should rely solely on this field to decide whether
	 * to show a preview button, rather than inferring paths from pp_execution itself
	 * (the frontend has no filesystem access and cannot verify a path actually exists).
	 */
	module_outputs?: Record<string, string>;
}
