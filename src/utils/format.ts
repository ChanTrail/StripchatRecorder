export function formatSize(bytes: number): string {
	if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
	if (bytes < 1024 ** 3) return (bytes / 1024 / 1024).toFixed(1) + " MB";
	return (bytes / 1024 / 1024 / 1024).toFixed(2) + " GB";
}

export function formatDuration(secs: number): string {
	const h = Math.floor(secs / 3600);
	const m = Math.floor((secs % 3600) / 60);
	const s = secs % 60;
	if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
	return `${m}:${String(s).padStart(2, "0")}`;
}
