//! 主播追踪记录的增删改查（`AppState` 方法，操作 `data.streamers`）。
//! CRUD for tracked-streamer records (`AppState` methods over `data.streamers`).

use super::{AppState, StreamerData};
use crate::core::error::{AppError, Result};

impl AppState {
	/// 获取所有追踪主播的克隆列表。
	/// Get a cloned list of all tracked streamers.
	pub fn get_streamers(&self) -> Vec<StreamerData> {
		self.data.read().streamers.clone()
	}

	/// 添加新主播到追踪列表（若已存在则返回错误）。
	/// `model_id` 通常来自添加时对 v1/broadcasts/{username} 的查询结果（见
	/// `streamer::add_streamer` 路由），用于日后改名反查；查询失败时传 `None`
	/// 也不阻塞添加，仅是后续改名跟随会失效。
	///
	/// Add a new streamer to the tracking list (returns error if already exists).
	/// `model_id` typically comes from the v1/broadcasts/{username} lookup performed at
	/// add time (see the `streamer::add_streamer` route), used later for rename lookups;
	/// passing `None` when that lookup fails doesn't block adding the streamer, it just
	/// means rename-following won't work for this entry.
	pub fn add_streamer(&self, username: &str, model_id: Option<i64>) -> Result<()> {
		let mut data = self.data.write();
		if data.streamers.iter().any(|s| s.username == username) {
			return Err(AppError::Other(format!("模特 {} 已存在", username)));
		}
		let auto_record = data.settings.auto_record;
		data.streamers.push(StreamerData {
			username: username.to_string(),
			auto_record,
			added_at: chrono::Local::now().to_rfc3339(),
			model_id,
			is_dead: false,
			schedule: None,
			cached_preview_url: None,
		});
		drop(data);
		self.save()
	}

	/// 回填指定主播的 model_id（仅当当前为 `None` 时才写入，避免覆盖已有值）。
	/// 用于升级前的旧数据在下一次成功查询时补齐 model_id。
	///
	/// Backfill the model_id for a streamer (only writes when currently `None`, to
	/// avoid overwriting an existing value). Used to backfill model_id for pre-upgrade
	/// data on its next successful lookup.
	pub fn backfill_model_id(&self, username: &str, model_id: i64) {
		let mut data = self.data.write();
		if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username)
			&& s.model_id.is_none()
		{
			s.model_id = Some(model_id);
			drop(data);
			let _ = self.save();
		}
	}

	/// 将主播记录从旧用户名重命名为新用户名（改名跟随），保留 model_id/auto_record/added_at
	/// 不变。若新用户名已存在于列表中（如同时手动添加过），则放弃改名，保留原记录不变，
	/// 避免产生两条指向同一 model_id 的重复记录。
	///
	/// Rename a streamer record from the old username to the new one (rename-following),
	/// preserving model_id/auto_record/added_at. If the new username already exists in
	/// the list (e.g. manually added separately), the rename is abandoned and the
	/// original record is left unchanged, avoiding two records pointing at the same
	/// model_id.
	pub fn rename_streamer(&self, old_username: &str, new_username: &str) -> Result<()> {
		let mut data = self.data.write();
		if data.streamers.iter().any(|s| s.username == new_username) {
			return Err(AppError::Other(format!(
				"无法改名：{} 已存在于追踪列表中",
				new_username
			)));
		}
		if let Some(s) = data
			.streamers
			.iter_mut()
			.find(|s| s.username == old_username)
		{
			s.username = new_username.to_string();
		}
		drop(data);
		self.save()
	}

	/// 将主播标记为已失效（is_dead = true）并保存。
	/// Mark a streamer as dead (is_dead = true) and save.
	pub fn mark_streamer_dead(&self, username: &str) {
		let mut data = self.data.write();
		if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username)
			&& !s.is_dead
		{
			s.is_dead = true;
			drop(data);
			let _ = self.save();
		}
	}

	/// 返回所有已标记为失效的主播用户名集合。
	/// Returns the set of all usernames marked as dead.
	pub fn get_dead_streamers(&self) -> std::collections::HashSet<String> {
		self.data
			.read()
			.streamers
			.iter()
			.filter(|s| s.is_dead)
			.map(|s| s.username.clone())
			.collect()
	}

	/// 从追踪列表中移除主播并保存。
	/// Remove a streamer from the tracking list and save.
	pub fn remove_streamer(&self, username: &str) -> Result<()> {
		let mut data = self.data.write();
		data.streamers.retain(|s| s.username != username);
		drop(data);
		self.save()
	}

	/// 设置指定主播的自动录制开关并保存。
	/// Set the auto-record toggle for a specific streamer and save.
	pub fn set_auto_record(&self, username: &str, enabled: bool) -> Result<()> {
		let mut data = self.data.write();
		if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username) {
			s.auto_record = enabled;
		}
		drop(data);
		self.save()
	}

	/// 保存从 camgirlfinder 获取的主播历史在线规律（7×48 矩阵）。
	/// 仅当主播存在时写入；若已有 schedule 则覆盖更新（schedule 会随时间变化）。
	///
	/// Persist the streamer's historical online schedule fetched from camgirlfinder
	/// (7×48 matrix). Only writes when the streamer exists; overwrites any existing
	/// schedule since it changes over time.
	pub fn set_schedule(&self, username: &str, schedule: Vec<Vec<f32>>) {
		let mut data = self.data.write();
		if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username) {
			s.schedule = Some(schedule);
			drop(data);
			let _ = self.save();
		}
	}

	/// 更新指定主播的离线预览图 URL 缓存（由状态轮询每天刷新时调用，与在线状态无关）。
	/// 仅当值确实发生变化时才写盘，避免无意义的 I/O。
	///
	/// Update the cached offline preview image URL for a streamer
	/// (called by status polling's daily refresh, regardless of online status).
	/// Only writes to disk when the value actually changes.
	pub fn set_cached_preview_url(&self, username: &str, url: Option<String>) {
		let mut data = self.data.write();
		if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username)
			&& s.cached_preview_url != url
		{
			s.cached_preview_url = url;
			drop(data);
			let _ = self.save();
		}
	}
}
