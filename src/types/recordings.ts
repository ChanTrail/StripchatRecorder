/**
 * 录制文件相关类型定义 / Recording File Type Definitions
 */

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
	/** 视频实际时长（秒），由 ffprobe 获取 / Actual video duration (seconds), obtained via ffprobe */
	video_duration_secs: number | null;
}
