use super::helpers::*;
use super::model::*;
use super::settings::SettingsHandle;
use super::*;

/// 一条命令的执行结果（不含块壳）。
pub(crate) struct Outcome {
    pub(crate) outputs: Vec<Output>,
    pub(crate) footer: Option<String>,
}

/// 执行器内部结果：普通命令已完成，`library scan` 则只是“已受理”。
pub(crate) enum Executed {
    /// 命令已执行完，`done` 块可立即推送。
    Finished(Outcome),
    /// 后台任务已受理；调用方只推 `running` 块，后续由观察者更新同一 id。
    Accepted(Outcome),
}

/// `execute` 的结果：空输入不做任何事，否则给出要按顺序推送的块。
pub enum Execution {
    /// 纯空白输入：不是错误，也不产生块。
    Ignored,
    /// 先推 `running`（空输出），再由 `done` 覆盖同一个 `id`。
    Blocks {
        /// 开始执行时推送的空块。
        running: Box<Block>,
        /// 执行结束的最终块。
        done: Box<Block>,
    },
    /// 后台命令已受理（`library scan`）：只推 `running` 块，最终块由观察者推送。
    Accepted {
        /// 已受理的块，输出里写明“扫描已开始”。
        running: Box<Block>,
    },
}

/// 解析并执行一条终端输入，返回可推送的块。
///
/// 空输入（`Ok(None)`）不产生块；解析失败也走 `running → error` 两个块，
/// 这样前端只要按 `id` 覆盖即可。
///
/// 播放器单测不需要真实媒体库：这条入口用空实现（搜索结果恒为空）。
#[cfg(test)]
pub fn execute(api: &dyn PlayerApi, block_id: &str, input: &str) -> Execution {
    execute_with_library(
        api,
        &EmptyLibraryApi,
        block_id,
        input,
        Arc::new(NoopLibraryObserver),
    )
}

/// 带媒体库的执行入口；`observer` 只被 `library scan` 使用。
///
/// `library scan` 的后台线程可能很快结束，所以 `cmd_exec` 会在调用本函数
/// **之前**先推 `running` 块，保证进度事件不会先于它到达前端。
#[cfg(test)]
pub fn execute_with_library(
    api: &dyn PlayerApi,
    library: &dyn LibraryApi,
    block_id: &str,
    input: &str,
    observer: Arc<dyn LibraryObserver>,
) -> Execution {
    execute_with_services(api, library, Services::default(), block_id, input, observer)
}

/// 带设置 / 主题 / 在线源依赖的执行入口（生产 `cmd_exec` 用）。
pub fn execute_with_services(
    api: &dyn PlayerApi,
    library: &dyn LibraryApi,
    services: Services<'_>,
    block_id: &str,
    input: &str,
    observer: Arc<dyn LibraryObserver>,
) -> Execution {
    let parsed = match parse(input) {
        Ok(None) => return Execution::Ignored,
        Ok(Some(parsed)) => Ok(parsed),
        Err(error) => Err(CmdError::from_parse(&error)),
    };

    let started_at = now_millis();
    let running = Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at,
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: Vec::new(),
        footer: None,
        schema_version: SCHEMA_VERSION,
        actions: None,
    };

    let clock = Instant::now();
    let listing = library.active_listing();
    let result = match &parsed {
        Ok(parsed) => Executor {
            api,
            library,
            observer,
            services,
            listing,
        }
        .run(parsed),
        // 引号未闭合这类错误定位不到命令，没有 usage / 建议，给一句通用提示
        // 保证失败块里的 `hint` 恒不为空（T-0018 §2.5）。
        Err(error) => Err(error.clone().with_hint("输入 help 查看全部命令与用法")),
    };
    let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);

    let done = match result {
        Ok(Executed::Accepted(outcome)) => {
            return Execution::Accepted {
                running: Box::new(Block {
                    outputs: outcome.outputs,
                    footer: outcome.footer,
                    ..running
                }),
            };
        }
        Ok(Executed::Finished(outcome)) => Block {
            duration_ms: Some(duration_ms),
            status: BlockStatus::Done,
            outputs: outcome.outputs,
            footer: outcome.footer,
            ..running.clone()
        },
        Err(error) => Block {
            duration_ms: Some(duration_ms),
            status: BlockStatus::Error,
            outputs: vec![error.to_output()],
            actions: error_actions(input, parsed.as_ref().ok()),
            ..running.clone()
        },
    };

    Execution::Blocks {
        running: Box::new(running),
        done: Box::new(done),
    }
}

/// 错误块动作（T-0045 阶段 3）：只生成真实存在的命令。
///
/// - 所有非空错误块都有「重试」（原命令）。
/// - 只有 `search` 且当前不是本地搜索时，追加「只用本地搜索」；
///   关键词来自 `SearchOptions`，不伪造不存在的「跳过该目录」。
pub fn error_actions(input: &str, parsed: Option<&Parsed>) -> Option<Vec<BlockAction>> {
    let retry = input.trim();
    if retry.is_empty() {
        return None;
    }

    let mut actions = vec![BlockAction {
        label: "重试".to_owned(),
        input: retry.to_owned(),
    }];

    if let Some(parsed) = parsed
        && parsed.spec.name == "search"
        && let Ok(options) = SearchOptions::parse(parsed)
        && !options.local
        && !options.keyword.trim().is_empty()
    {
        actions.push(BlockAction {
            label: "只用本地搜索".to_owned(),
            input: format!(
                "search {} --local",
                quote_search_keyword(options.keyword.trim())
            ),
        });
    }

    Some(actions)
}

/// 关键词含空白/引号时用双引号包住，保证动作命令仍是一个 search 查询。
fn quote_search_keyword(keyword: &str) -> String {
    if keyword
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '"' || ch == '\\')
    {
        format!("\"{}\"", keyword.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        keyword.to_owned()
    }
}

/// 设置/主题/在线源这几类命令的依赖；旧测试可以全部传 `None`。
#[derive(Clone, Copy, Default)]
pub struct Services<'a> {
    pub settings: Option<&'a SettingsHandle>,
    pub theme: Option<&'a dyn ThemeApi>,
    pub online: Option<&'a dyn OnlineApi>,
}

/// 把 [`Parsed`] 映射成 [`PlayerApi`] / [`LibraryApi`] 调用并构造输出。
pub(crate) struct Executor<'a> {
    api: &'a dyn PlayerApi,
    library: &'a dyn LibraryApi,
    observer: Arc<dyn LibraryObserver>,
    services: Services<'a>,
    listing: Option<ActiveListing>,
}

impl Executor<'_> {
    /// 执行一条已解析的命令；失败时补上命令用法作为提示。
    pub(crate) fn run(&self, parsed: &Parsed) -> Result<Executed, CmdError> {
        if is_library_scan(parsed) {
            return self
                .library_scan(parsed)
                .map(Executed::Accepted)
                .map_err(|error| error.with_usage(parsed));
        }
        self.dispatch(parsed)
            .map(Executed::Finished)
            .map_err(|error| error.with_usage(parsed))
    }

    pub(crate) fn dispatch(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        match parsed.spec.name {
            "play" => self.play(parsed),
            "pause" => self.simple("暂停", self.api.pause()),
            "resume" => self.simple("继续播放", self.api.resume()),
            "next" => self.simple("下一首", self.api.next()),
            "prev" => self.simple("上一首", self.api.prev()),
            "seek" => self.seek(parsed),
            "vol" => self.volume(parsed),
            "queue" => self.queue(parsed),
            "mode" => self.mode(parsed),
            "library" => self.library(parsed),
            "playlist" => self.playlist(parsed),
            "search" => self.search(parsed),
            "theme" => self.theme(parsed),
            "config" => self.config(parsed),
            "source" => self.source(parsed),
            "cache" => self.cache(parsed),
            "help" => self.help(parsed),
            "version" => Ok(Self::version()),
            "clear" => Err(CmdError::new("E_CMD_ARG", "clear 由终端本地处理")
                .with_hint("clear 是视图操作，前端直接清屏，不经过后端")),
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("命令表与执行器不一致：{other}"),
            )),
        }
    }

    /// `play`：无参 = 继续；纯数字 = 按**最近一张带序号表格**解析；其余 = 文件路径。
    ///
    /// 没有列表上下文时不再回退到播放队列（T-0043 统一规则）。
    fn play(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let Some(value) = parsed.positional.first() else {
            return Ok(self.state_outcome("继续播放", &self.api.resume()?));
        };

        if is_unsigned_integer(value) {
            let index = user_index(value)?;
            let listing = self.listing.clone().ok_or_else(|| {
                CmdError::new("E_CMD_ARG", "没有可用的序号列表")
                    .with_hint("先执行 search / queue list 等命令，再用序号")
            })?;

            match listing.source {
                ListingSource::Search => {
                    let hits = self.library.search_hits();
                    if index >= listing.len || index >= hits.len() {
                        return Err(CmdError::new(
                            "E_CMD_ARG",
                            format!(
                                "{}只有 {} 条，没有第 {} 条",
                                listing.source.label(),
                                listing.len.max(hits.len()),
                                index + 1
                            ),
                        ));
                    }
                    let hit = &hits[index];
                    let SearchHit::Local(track) = hit else {
                        // 正常路径由 cmd_exec 的在线分派处理；这里只防御直接调用执行器的场景。
                        return Err(CmdError::new(
                            "E_SRC_UNAVAILABLE",
                            "该结果是云曲目，请重新执行 play <序号> 以走在线下载",
                        ));
                    };
                    let state = self.api.play(Some(track.path.clone()))?;
                    let mut spans = vec![
                        bold("播放"),
                        plain(format!(" · 搜索结果第 {} 首", index + 1)),
                        dim(format!(" · {}", track.title)),
                    ];
                    if !track.artist.is_empty() {
                        spans.push(dim(format!(" · {}", track.artist)));
                    }
                    return Ok(Outcome {
                        outputs: vec![text_of(spans)],
                        footer: Some(format!(
                            "来源：搜索结果（共 {} 条） · {}",
                            listing.len.max(hits.len()),
                            state_footer(&state)
                        )),
                    });
                }
                ListingSource::Queue => {
                    let view = self.api.queue_list()?;
                    if index >= listing.len || view.tracks.get(index).is_none() {
                        return Err(CmdError::new(
                            "E_CMD_ARG",
                            format!(
                                "{}只有 {} 首，没有第 {} 首",
                                listing.source.label(),
                                listing.len.max(view.tracks.len()),
                                index + 1
                            ),
                        ));
                    }
                    let view = self.api.queue_jump(index)?;
                    return Ok(self.queue_jump_outcome(index, &view));
                }
                ListingSource::Library => {
                    let tracks = self.library.library_listing();
                    let Some(track) = tracks.get(index) else {
                        return Err(CmdError::new(
                            "E_CMD_ARG",
                            format!(
                                "{}只有 {} 条，没有第 {} 条",
                                listing.source.label(),
                                listing.len.max(tracks.len()),
                                index + 1
                            ),
                        ));
                    };
                    let state = self.api.play(Some(track.path.clone()))?;
                    let mut spans = vec![
                        bold("播放"),
                        plain(format!(" · 媒体库第 {} 首", index + 1)),
                        dim(format!(" · {}", track.title)),
                    ];
                    if !track.artist.is_empty() {
                        spans.push(dim(format!(" · {}", track.artist)));
                    }
                    return Ok(Outcome {
                        outputs: vec![text_of(spans)],
                        footer: Some(format!(
                            "来源：媒体库（共 {} 首） · {}",
                            listing.len.max(tracks.len()),
                            state_footer(&state)
                        )),
                    });
                }
                ListingSource::Playlist(name) => {
                    let playlist = self.playlist_by_name(&name)?;
                    return self.play_playlist_item(&playlist, index);
                }
            }
        }

        Ok(self.state_outcome("播放", &self.api.play(Some(value.clone()))?))
    }

    /// 给人看的曲目名：复用库/远程会话映射；miss 回退文件名。
    fn describe_track(&self, path: &str) -> String {
        match self.library.track_by_path(path) {
            Ok(Some(track)) if !track.title.trim().is_empty() => {
                if track.artist.trim().is_empty() {
                    track.title.trim().to_owned()
                } else {
                    format!("{} · {}", track.title.trim(), track.artist.trim())
                }
            }
            _ => track_display(path),
        }
    }

    fn state_outcome(&self, headline: &str, state: &PlaybackState) -> Outcome {
        let mut spans = vec![bold(headline)];
        if let Some(track) = state.track_id.as_deref() {
            spans.push(dim(format!(" · {}", self.describe_track(track))));
            let position = format_position(state.position_ms);
            let duration = state
                .duration_ms
                .map_or_else(|| "--:--".to_owned(), format_position);
            spans.push(dim(format!(" · {position} / {duration}")));
        }
        let footer = match state.queue_length {
            Some(length) if length > 0 => Some(format!("队列 {length} 首")),
            _ => None,
        };

        Outcome {
            outputs: vec![text_of(spans)],
            footer,
        }
    }

    fn jump_outcome(&self, headline: &str, index: usize, view: &QueueView) -> Outcome {
        let mut spans = vec![bold(headline), plain(format!(" · 第 {} 首", index + 1))];
        if let Some(track) = view.tracks.get(index) {
            spans.push(dim(format!(" · {}", self.describe_track(track))));
        }
        Outcome {
            outputs: vec![text_of(spans)],
            footer: Some(format!("队列 {} 首", view.tracks.len())),
        }
    }

    fn queue_jump_outcome(&self, index: usize, view: &QueueView) -> Outcome {
        let mut outcome = self.jump_outcome("播放", index, view);
        outcome.footer = Some(format!("来源：播放队列（共 {} 首）", view.tracks.len()));
        outcome
    }

    /// 播放类命令的公共收尾：一句标题 + 当前曲目 / 位置。
    fn simple(
        &self,
        headline: &str,
        result: Result<PlaybackState, CmdError>,
    ) -> Result<Outcome, CmdError> {
        Ok(self.state_outcome(headline, &result?))
    }

    /// `seek`：`mm:ss` / 纯毫秒是绝对位置，`±N`（秒）与 `±mm:ss` 相对当前位置。
    fn seek(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let raw = positional_arg(parsed, "播放位置")?;
        let target = if raw.starts_with('+') || raw.starts_with('-') {
            let state = self.api.snapshot()?;
            parse_relative_seek(raw, state.position_ms, state.duration_ms)?
        } else {
            parse_absolute_seek(raw)?
        };

        Ok(self.state_outcome("跳转", &self.api.seek(target)?))
    }

    /// `vol`：`±N` 先读当前音量；无论哪种写法都在调用前钳到 0–100。
    fn volume(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let raw = positional_arg(parsed, "音量")?;
        let level = if raw.starts_with('+') || raw.starts_with('-') {
            let state = self.api.snapshot()?;
            relative_volume(raw, state.volume)?
        } else {
            raw.parse::<i64>()
                .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的音量：{raw}")))?
        };
        let clamped = level.clamp(0, 100);
        self.api.volume(clamped as f32 / 100.0)?;
        if let Some(settings) = self.services.settings {
            settings.set_and_save(
                "playback.volume",
                np_core::SettingValue::Num(clamped as f64 / 100.0),
            )?;
        }

        Ok(Outcome {
            outputs: vec![text_of(vec![bold("音量"), plain(format!(" · {clamped}%"))])],
            footer: None,
        })
    }

    /// `mode`：先取当前模式，再一次性下发完整的 `{ repeat, shuffle }`。
    fn mode(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "mode 缺少子命令"))?;
        let raw = positional_arg(parsed, "模式取值")?;
        let current = self.api.snapshot()?.mode;

        let mode = match subcommand.name {
            "repeat" => PlaybackMode {
                repeat: repeat_of(raw),
                shuffle: current.shuffle,
            },
            "shuffle" => PlaybackMode {
                repeat: current.repeat,
                shuffle: raw == "on",
            },
            other => {
                return Err(CmdError::new(
                    "E_CMD_UNKNOWN",
                    format!("mode 没有子命令：{other}"),
                ));
            }
        };

        let state = self.api.mode(mode)?;
        if let Some(settings) = self.services.settings {
            settings.set_and_save(
                "playback.mode",
                SettingValue::Str(super::settings::playback_mode_id(state.mode).to_owned()),
            )?;
        }
        Ok(Outcome {
            outputs: vec![text_of(mode_spans(&state.mode))],
            footer: None,
        })
    }
    /// `queue`：`list` / `add` / `remove` / `clear` / `jump`。
    fn queue(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "queue 缺少子命令"))?;

        match subcommand.name {
            "list" => self.queue_list(),
            "add" => self.add_paths(parsed),
            "remove" => {
                let index = user_index(positional_arg(parsed, "队列序号")?)?;
                let view = self.api.queue_remove(index)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已移除"),
                        plain(format!(" · 第 {} 首", index + 1)),
                    ])],
                    footer: Some(format!("队列还剩 {} 首", view.tracks.len())),
                })
            }
            "clear" => {
                let view = self.api.queue_clear()?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![bold("队列已清空")])],
                    footer: Some(format!("队列 {} 首", view.tracks.len())),
                })
            }
            "jump" => {
                let index = user_index(positional_arg(parsed, "队列序号")?)?;
                let view = self.api.queue_jump(index)?;
                Ok(self.jump_outcome("跳转", index, &view))
            }
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("queue 没有子命令：{other}"),
            )),
        }
    }

    /// `queue list`：元数据来自一次批量查库 + 远程会话映射；查不到回退文件名 + —。
    fn queue_list(&self) -> Result<Outcome, CmdError> {
        let view = self.api.queue_list()?;
        let metadata = self.library.tracks_by_paths(&view.tracks)?;
        let mut rows = Vec::with_capacity(view.tracks.len());
        let mut has_source = false;

        for path in &view.tracks {
            let filename = track_display(path);
            let Some(track) = metadata.get(path) else {
                rows.push(QueueRow {
                    title: filename,
                    artist: "—".to_owned(),
                    duration: "—".to_owned(),
                    source: None,
                });
                continue;
            };

            let title = if track.title.trim().is_empty() {
                filename
            } else {
                track.title.clone()
            };
            let artist = if track.artist.trim().is_empty() {
                "—".to_owned()
            } else {
                track.artist.clone()
            };
            let duration = if track.duration_ms > 0 {
                format_track_duration(track.duration_ms)
            } else {
                "—".to_owned()
            };
            let source = queue_track_source(track);
            has_source |= source.is_some();
            rows.push(QueueRow {
                title,
                artist,
                duration,
                source,
            });
        }

        let mut columns = vec![
            "序号".to_owned(),
            "标题".to_owned(),
            "艺人".to_owned(),
            "时长".to_owned(),
        ];
        if has_source {
            columns.push("来源".to_owned());
        }
        let table_rows = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                let sequence = if view.index == Some(index) {
                    format!("▶ {}", index + 1)
                } else {
                    (index + 1).to_string()
                };
                let mut cells = vec![sequence, row.title, row.artist, row.duration];
                if has_source {
                    cells.push(row.source.unwrap_or_else(|| "—".to_owned()));
                }
                cells
            })
            .collect();

        let footer = if view.tracks.is_empty() {
            "队列为空 · 用 queue add <路径> 加入曲目".to_owned()
        } else {
            format!("{} 首 · 输入 play 1 开始播放", view.tracks.len())
        };
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Queue,
            len: view.tracks.len(),
        });

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns,
                rows: table_rows,
                selectable: true,
            }],
            footer: Some(footer),
        })
    }

    /// `queue add <path>` 收窄为真实存在的本地文件；`--from-search <n>` 从最近搜索结果入队。
    fn add_paths(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let previous_listing = self.listing.clone();
        let from_search = parsed.flags.get("from-search");

        let path = if let Some(raw) = from_search {
            if !parsed.positional.is_empty() {
                return Err(CmdError::new(
                    "E_CMD_ARG",
                    "queue add 只能在 <path> 与 --from-search <n> 中二选一",
                )
                .with_hint("queue add <path> | queue add --from-search <n>"));
            }
            let index = user_index(raw)?;
            let listing = self.listing.clone().ok_or_else(|| {
                CmdError::new("E_CMD_ARG", "没有可用的序号列表")
                    .with_hint("先执行 search / queue list 等命令，再用序号")
            })?;
            if listing.source != ListingSource::Search {
                return Err(CmdError::new("E_CMD_ARG", "当前序号列表不是搜索结果")
                    .with_hint("先执行 search <关键词>，再用 queue add --from-search <n>"));
            }
            let hits = self.library.search_hits();
            let Some(hit) = hits.get(index) else {
                return Err(CmdError::new(
                    "E_CMD_ARG",
                    format!(
                        "{}只有 {} 条，没有第 {} 条",
                        listing.source.label(),
                        listing.len.max(hits.len()),
                        index + 1
                    ),
                )
                .with_hint("先执行 search <关键词>"));
            };
            match hit {
                SearchHit::Local(track) => track.path.clone(),
                SearchHit::Online(track) => {
                    let online = self.services.online.ok_or_else(|| {
                        CmdError::new("E_SRC_UNAVAILABLE", "在线搜索结果入队需要在线服务")
                    })?;
                    // 队列只存本地路径：先把在线曲目解析/下载到缓存，再入队。
                    let prepared = online.prepare(track, &mut |_| {})?;
                    prepared.path.to_string_lossy().into_owned()
                }
            }
        } else {
            if parsed.positional.is_empty() {
                return Err(CmdError::new("E_CMD_ARG", "缺少参数：路径")
                    .with_hint("queue add <路径> | queue add --from-search <序号>"));
            }
            if parsed.positional.len() > 1 {
                return Err(CmdError::new("E_CMD_ARG", "queue add 一次只接受一个路径")
                    .with_hint("queue add <路径> | queue add --from-search <序号>"));
            }
            let path = parsed.positional[0].clone();
            if !Path::new(&path).is_file() {
                return Err(CmdError::new("E_CMD_ARG", format!("文件不存在：{path}"))
                    .with_hint("queue add 只接受本地文件；搜索结果用 --from-search <n>"));
            }
            path
        };

        let view = self.api.queue_add(vec![path])?;
        let mut footer = format!("队列现有 {} 首", view.tracks.len());
        if let Some(listing) = previous_listing {
            footer.push_str(&format!(
                " · 现在 play <n> 仍指向{}；要播队列先 queue list",
                listing.source.label()
            ));
        }
        Ok(Outcome {
            outputs: vec![text_of(vec![bold("已加入 1 首")])],
            footer: Some(footer),
        })
    }

    /// `playlist`：8 个子命令；所有名字按 COLLATE NOCASE 解析。
    fn playlist(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "playlist 缺少子命令"))?;
        match subcommand.name {
            "list" => {
                let playlists = self.library.playlist_list()?;
                if playlists.is_empty() {
                    return Ok(Outcome {
                        outputs: vec![text_of(vec![
                            bold("还没有歌单"),
                            dim(" · playlist create \"名字\" 新建"),
                        ])],
                        footer: None,
                    });
                }
                let rows = playlists
                    .iter()
                    .map(|playlist| vec![playlist.name.clone(), playlist.item_count.to_string()])
                    .collect();
                Ok(Outcome {
                    outputs: vec![Output::Table {
                        columns: vec!["歌单".to_owned(), "条目".to_owned()],
                        rows,
                        selectable: false,
                    }],
                    footer: Some("playlist show \"名字\" 查看条目".to_owned()),
                })
            }
            "create" => {
                let name = positional_arg(parsed, "名字")?;
                self.library.playlist_create(name)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已创建歌单"),
                        plain(format!(" · {name}")),
                    ])],
                    footer: Some("playlist add \"名字\" <序号> 加入当前列表条目".to_owned()),
                })
            }
            "rename" => {
                let old = positional_arg(parsed, "旧名")?;
                let new = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| {
                        CmdError::new("E_CMD_ARG", "缺少参数：新名")
                            .with_hint("名字含空格时用引号：playlist rename \"旧名\" \"新名\"")
                    })?;
                let playlist = self.playlist_by_name(old)?;
                self.library.playlist_rename(playlist.id, new)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已重命名"),
                        plain(format!(" · {old} → {new}")),
                    ])],
                    footer: None,
                })
            }
            "delete" => {
                let name = positional_arg(parsed, "名字")?;
                let playlist = self.playlist_by_name(name)?;
                self.library.playlist_delete(playlist.id)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已删除歌单"),
                        plain(format!(" · {name}")),
                    ])],
                    footer: None,
                })
            }
            "show" => self.playlist_show(positional_arg(parsed, "名字")?),
            "add" => {
                let name = positional_arg(parsed, "名字")?;
                match parsed.positional.get(1) {
                    Some(raw_index) => self.playlist_add(name, user_index(raw_index)?),
                    None => self.playlist_add_current(name),
                }
            }
            "remove" => {
                let name = positional_arg(parsed, "名字")?;
                let raw_index = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| CmdError::new("E_CMD_ARG", "缺少参数：序号"))?;
                let index = user_index(raw_index)?;
                let playlist = self.playlist_by_name(name)?;
                self.library
                    .playlist_remove(playlist.id, i64::try_from(index).unwrap_or(i64::MAX))?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已从歌单移除"),
                        plain(format!(" · {name} 第 {} 首", index + 1)),
                    ])],
                    footer: None,
                })
            }
            "play" => {
                let name = positional_arg(parsed, "名字")?;
                let index = match parsed.positional.get(1) {
                    Some(raw) => user_index(raw)?,
                    None => 0,
                };
                let playlist = self.playlist_by_name(name)?;
                self.play_playlist_item(&playlist, index)
            }
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("playlist 没有子命令：{other}"),
            )),
        }
    }

    fn playlist_by_name(&self, name: &str) -> Result<Playlist, CmdError> {
        self.library.playlist_by_name(name)?.ok_or_else(|| {
            CmdError::new("E_CMD_ARG", format!("歌单不存在：{name}"))
                .with_hint("名字含空格时用引号：playlist show \"名字\"")
        })
    }

    fn playlist_show(&self, name: &str) -> Result<Outcome, CmdError> {
        let playlist = self.playlist_by_name(name)?;
        let (_, items) = self.library.playlist_get(playlist.id)?;
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Playlist(playlist.name.clone()),
            len: items.len(),
        });
        if items.is_empty() {
            return Ok(Outcome {
                outputs: vec![text_of(vec![
                    bold(format!("歌单：{}", playlist.name)),
                    dim(" · 0 首"),
                ])],
                footer: Some("playlist add \"名字\" <序号> 加入条目".to_owned()),
            });
        }
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let missing = item.kind == "local" && !Path::new(&item.path).is_file();
                let title = if missing {
                    format!("! {}", item.title)
                } else {
                    item.title.clone()
                };
                vec![
                    (index + 1).to_string(),
                    title,
                    item.artist.clone(),
                    if item.duration_ms > 0 {
                        format_track_duration(item.duration_ms)
                    } else {
                        "—".to_owned()
                    },
                    playlist_item_source(item),
                ]
            })
            .collect();
        Ok(Outcome {
            outputs: vec![
                text_of(vec![
                    bold(format!("歌单：{}", playlist.name)),
                    dim(format!(" · {} 首", items.len())),
                ]),
                Output::Table {
                    columns: vec![
                        "序号".to_owned(),
                        "标题".to_owned(),
                        "艺人".to_owned(),
                        "时长".to_owned(),
                        "来源".to_owned(),
                    ],
                    rows,
                    selectable: true,
                },
            ],
            footer: Some("输入 play <序号> 播放".to_owned()),
        })
    }

    fn playlist_add(&self, name: &str, index: usize) -> Result<Outcome, CmdError> {
        let listing = self.listing.clone().ok_or_else(|| {
            CmdError::new("E_CMD_ARG", "当前没有可用的列表")
                .with_hint("先 search / queue list / playlist show，或先播放一首")
        })?;
        if index >= listing.len {
            return Err(CmdError::new(
                "E_CMD_ARG",
                format!(
                    "{}只有 {} 条，没有第 {} 条",
                    listing.source.label(),
                    listing.len,
                    index + 1
                ),
            ));
        }
        let draft = self.listing_draft(&listing, index)?;
        self.add_draft(name, draft)
    }

    fn playlist_add_current(&self, name: &str) -> Result<Outcome, CmdError> {
        let draft = self.current_playing_draft()?;
        self.add_draft(name, draft)
    }

    fn current_playing_draft(&self) -> Result<NewPlaylistItem, CmdError> {
        let state = self.api.snapshot()?;
        let path = state.track_id.ok_or_else(|| {
            CmdError::new("E_CMD_ARG", "当前没有正在播放的曲目")
                .with_hint("先 search / queue list / playlist show，或先播放一首")
        })?;
        Ok(match self.library.track_by_path(&path)? {
            Some(track) if track.kind == "remote" => remote_track_draft(&track),
            Some(track) => local_draft(&track),
            None => NewPlaylistItem {
                kind: "local".to_owned(),
                path: path.clone(),
                source: String::new(),
                source_id: String::new(),
                title: track_display(&path),
                artist: "—".to_owned(),
                album: String::new(),
                duration_ms: 0,
            },
        })
    }

    fn add_draft(&self, name: &str, draft: NewPlaylistItem) -> Result<Outcome, CmdError> {
        let playlist = self.playlist_by_name(name)?;
        let (_, items) = self.library.playlist_get(playlist.id)?;
        let duplicate = items.iter().any(|item| same_playlist_item(&draft, item));
        let added = self.library.playlist_add(playlist.id, &draft)?;
        let mut spans = vec![
            bold("已加入歌单"),
            plain(format!(" · {} 第 {} 首", playlist.name, added.position + 1)),
            dim(format!(" · {}", added.title)),
        ];
        if duplicate {
            spans.push(dim(" · 已在歌单中"));
        }
        Ok(Outcome {
            outputs: vec![text_of(spans)],
            footer: None,
        })
    }

    fn listing_draft(
        &self,
        listing: &ActiveListing,
        index: usize,
    ) -> Result<NewPlaylistItem, CmdError> {
        match &listing.source {
            ListingSource::Search => {
                let hits = self.library.search_hits();
                let hit = hits.get(index).ok_or_else(|| {
                    CmdError::new("E_CMD_ARG", format!("搜索结果只有 {} 条", hits.len()))
                })?;
                Ok(match hit {
                    SearchHit::Local(track) => local_draft(track),
                    SearchHit::Online(track) => remote_draft(track),
                })
            }
            ListingSource::Queue => {
                let view = self.api.queue_list()?;
                let path = view.tracks.get(index).ok_or_else(|| {
                    CmdError::new(
                        "E_CMD_ARG",
                        format!("播放队列只有 {} 首", view.tracks.len()),
                    )
                })?;
                let metadata = self.library.tracks_by_paths(std::slice::from_ref(path))?;
                Ok(match metadata.get(path) {
                    Some(track) => local_draft(track),
                    None => NewPlaylistItem {
                        kind: "local".to_owned(),
                        path: path.clone(),
                        source: String::new(),
                        source_id: String::new(),
                        title: track_display(path),
                        artist: "—".to_owned(),
                        album: String::new(),
                        duration_ms: 0,
                    },
                })
            }
            ListingSource::Library => {
                let tracks = self.library.library_listing();
                let track = tracks.get(index).ok_or_else(|| {
                    CmdError::new("E_CMD_ARG", format!("媒体库只有 {} 条", tracks.len()))
                })?;
                Ok(local_draft(track))
            }
            ListingSource::Playlist(name) => {
                let playlist = self.playlist_by_name(name)?;
                let (_, items) = self.library.playlist_get(playlist.id)?;
                let item = items.get(index).ok_or_else(|| {
                    CmdError::new(
                        "E_CMD_ARG",
                        format!("歌单《{name}》只有 {} 首", items.len()),
                    )
                })?;
                Ok(playlist_draft(item))
            }
        }
    }

    fn play_playlist_item(&self, playlist: &Playlist, index: usize) -> Result<Outcome, CmdError> {
        let (_, items) = self.library.playlist_get(playlist.id)?;
        let item = items.get(index).ok_or_else(|| {
            CmdError::new(
                "E_CMD_ARG",
                format!("歌单《{}》里只有 {} 首", playlist.name, items.len()),
            )
        })?;
        let mut paths = Vec::with_capacity(items.len());
        for item in &items {
            if item.kind == "local" {
                if !Path::new(&item.path).is_file() {
                    return Err(CmdError::new("E_CMD_ARG", "文件不存在，可能已移动"));
                }
                paths.push(item.path.clone());
            } else {
                let source = source_from_str(&item.source)?;
                let track = SourceTrack {
                    source,
                    source_id: item.source_id.clone(),
                    title: item.title.clone(),
                    artist: item.artist.clone(),
                    duration_ms: u64::try_from(item.duration_ms).unwrap_or(0),
                    album: (!item.album.is_empty()).then(|| item.album.clone()),
                    cover_url: None,
                };
                let online = self
                    .services
                    .online
                    .ok_or_else(|| CmdError::new("E_SRC_UNAVAILABLE", "在线服务未接入"))?;
                let prepared = online.prepare(&track, &mut |_| {})?;
                paths.push(prepared.path.to_string_lossy().into_owned());
            }
        }
        self.api.queue_clear()?;
        self.api.queue_add(paths)?;
        self.api.queue_jump(index)?;
        let state = self.api.snapshot()?;
        Ok(Outcome {
            outputs: vec![text_of(vec![
                bold("播放"),
                plain(format!(" · {} 第 {} 首", playlist.name, index + 1)),
                dim(format!(" · {}", item.title)),
                dim(format!(" · 来源：{}", playlist_item_source(item))),
            ])],
            footer: Some(state_footer(&state)),
        })
    }

    /// `library`：`add` / `list` / `stat`（`scan` 在 `run` 里提前处理）。
    fn library(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "library 缺少子命令"))?;

        match subcommand.name {
            "add" => {
                let path = positional_arg(parsed, "目录")?;
                let roots = self.library.add_root(path)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已添加扫描根"),
                        dim(format!(" · {path}")),
                    ])],
                    footer: Some(format!(
                        "当前 {roots} 个扫描根 · 输入 library scan 开始扫描"
                    )),
                })
            }
            "list" => self.library_list(parsed),
            "stat" => self.library_stat(),
            "scan" => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                "library scan 应由后台扫描入口处理",
            )),
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("library 没有子命令：{other}"),
            )),
        }
    }

    /// `library list [--limit=N]`：最近入库的曲目，序号 1 基。
    fn library_list(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let limit = flag_limit(parsed, 20)?;
        let tracks = self.library.list_recent(limit)?;
        let rows = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                vec![
                    (index + 1).to_string(),
                    track.title.clone(),
                    track.artist.clone(),
                    format_track_duration(track.duration_ms),
                ]
            })
            .collect();
        self.library.remember_library_listing(tracks.clone());
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Library,
            len: tracks.len(),
        });

        let footer = if tracks.is_empty() {
            "库里还没有曲目 · 先 library add <path> 再 library scan".to_owned()
        } else {
            format!(
                "最近 {} 首 · 输入 search <关键词> 可按序号播放",
                tracks.len()
            )
        };

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns: vec![
                    "序号".to_owned(),
                    "标题".to_owned(),
                    "艺人".to_owned(),
                    "时长".to_owned(),
                ],
                rows,
                selectable: true,
            }],
            footer: Some(footer),
        })
    }

    /// `library stat`：曲目 / 专辑 / 艺人 / 总时长 / 最近扫描 / 封面缓存。
    fn library_stat(&self) -> Result<Outcome, CmdError> {
        let stats = self.library.stats()?;
        let last_scan = if stats.last_scan_at == 0 {
            "从未扫描".to_owned()
        } else {
            format_relative_time(stats.last_scan_at)
        };
        let rows = vec![
            vec![
                "曲目".to_owned(),
                stats.tracks.to_string(),
                "专辑".to_owned(),
                stats.albums.to_string(),
            ],
            vec![
                "艺人".to_owned(),
                stats.artists.to_string(),
                "总时长".to_owned(),
                format_total_duration(stats.total_duration_ms),
            ],
            vec![
                "最近扫描".to_owned(),
                last_scan,
                "缓存".to_owned(),
                "streams".to_owned(),
            ],
        ];

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns: ["项目", "值", "项目", "值"]
                    .iter()
                    .map(|column| (*column).to_owned())
                    .collect(),
                rows,
                selectable: false,
            }],
            footer: Some("输入 library list 查看最近入库".to_owned()),
        })
    }

    /// `search`：本地库搜索（在线分派由 `maybe_start_online` 处理）。
    fn search(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let options = SearchOptions::parse(parsed)?;
        let keyword = &options.keyword;
        let mut tracks = self.library.search(keyword, options.local_fetch_limit())?;
        let before_artist = tracks.len();
        if let Some(artist) = &options.artist {
            tracks.retain(|track| !matches!(local_artist_match(track, artist), MatchKind::None));
        }
        let removed = before_artist.saturating_sub(tracks.len());
        let truncated = tracks.len() > options.limit;
        tracks.truncate(options.limit);
        self.library.remember_search(tracks.clone());

        if tracks.is_empty() {
            // 空搜索也是一张（零行）Search 表：清掉旧上下文，避免 play 播到上一次结果。
            self.library.set_active_listing(ActiveListing {
                source: ListingSource::Search,
                len: 0,
            });
            let mut outputs = vec![text_of(vec![
                bold("没有找到"),
                dim(format!(" · {keyword}")),
            ])];
            if options.artist.is_some() && before_artist > 0 && removed > 0 {
                outputs.push(hint_output(&format!(
                    "过滤前有 {before_artist} 条，被 --artist 过滤掉了 {removed} 条"
                )));
            } else {
                outputs.push(hint_output("换个关键词试试"));
            }
            return Ok(Outcome {
                outputs,
                footer: Some(options.footer(0, false)),
            });
        }
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: tracks.len(),
        });

        let rows = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                vec![
                    (index + 1).to_string(),
                    track.title.clone(),
                    track.artist.clone(),
                    format_track_duration(track.duration_ms),
                ]
            })
            .collect();
        let mut outputs = vec![
            text_of(vec![bold("搜索结果"), dim(format!(" · {keyword}"))]),
            Output::Table {
                columns: vec![
                    "序号".to_owned(),
                    "标题".to_owned(),
                    "艺人".to_owned(),
                    "时长".to_owned(),
                ],
                rows,
                selectable: true,
            },
        ];
        if truncated {
            outputs.push(text_of(vec![dim(format!(
                "结果超过 {} 条，只显示前 {} 条",
                options.limit, options.limit
            ))]));
        }

        Ok(Outcome {
            outputs,
            footer: Some(options.footer(tracks.len(), false)),
        })
    }

    /// `library scan [--full]`：受理后台扫描并立即返回（`ok` = 已受理）。
    fn library_scan(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let full = parsed.flags.contains_key("full");
        self.library.scan(full, Arc::clone(&self.observer))?;
        let (outputs, footer) = scan_started_output(full);

        Ok(Outcome { outputs, footer })
    }

    /// `theme`：列出 / 切换 / 导出。切换会写 settings 并广播 `theme:changed`。
    fn theme(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let theme = self
            .services
            .theme
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "主题服务未接入"))?;
        let settings = self
            .services
            .settings
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "设置服务未接入"))?;

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("set") => {
                let id = positional_arg(parsed, "主题 id")?;
                let applied = theme.apply(id).map_err(normalize_cmd_dots)?;
                settings.set_and_save("theme.id", SettingValue::Str(applied.id.clone()))?;
                theme.broadcast(&applied.id);

                let source = match applied.source {
                    ThemeSource::Builtin => "内置",
                    ThemeSource::User => "用户",
                };
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("主题已切换"),
                        plain(format!(" · {}", applied.name)),
                        dim(format!(" · {} · {source}", applied.id)),
                    ])],
                    footer: Some("重启后仍会恢复到这个主题".to_owned()),
                })
            }
            Some("export") => {
                let id = positional_arg(parsed, "主题 id")?;
                let target = theme.export(id).map_err(normalize_cmd_dots)?;
                let detail = if target.created {
                    "已导出"
                } else {
                    "文件已存在，未覆盖"
                };
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("主题导出"),
                        plain(format!(" · {detail}")),
                        dim(format!(" · {}", target.path)),
                    ])],
                    footer: Some("编辑 JSON 后可用 theme set 应用".to_owned()),
                })
            }
            _ => {
                let entries = theme.list()?;
                let current = settings.theme_id().unwrap_or_else(|| "graphite".to_owned());
                let rows = entries
                    .iter()
                    .map(|entry| {
                        vec![
                            if entry.id == current {
                                "●".to_owned()
                            } else {
                                String::new()
                            },
                            entry.id.clone(),
                            entry.name.clone(),
                            match entry.source {
                                ThemeSource::Builtin => "内置".to_owned(),
                                ThemeSource::User => "用户".to_owned(),
                            },
                            entry.skin.as_str().to_owned(),
                            BuiltinTheme::find(&entry.id)
                                .map(|theme| theme.structure.summary())
                                .unwrap_or_else(|| "自定义".to_owned()),
                        ]
                    })
                    .collect();
                Ok(Outcome {
                    outputs: vec![
                        text_of(vec![bold("主题"), dim(format!(" · 当前：{current}"))]),
                        Output::Table {
                            columns: vec![
                                "当前".to_owned(),
                                "id".to_owned(),
                                "名称".to_owned(),
                                "来源".to_owned(),
                                "skin".to_owned(),
                                "结构".to_owned(),
                            ],
                            rows,
                            selectable: true,
                        },
                    ],
                    footer: Some("theme set <id> 切换 · theme export <id> 导出样例".to_owned()),
                })
            }
        }
    }

    /// `config`：list / get / set / unset / reset，全部读写同一份 settings.json。
    fn config(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let settings = self
            .services
            .settings
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "设置服务未接入"))?;

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("get") => {
                let key = positional_arg(parsed, "key")?;
                let value = settings.get(key).ok_or_else(|| {
                    CmdError::new("E_CMD_ARG", format!("未设置：{key}"))
                        .with_hint(format!("config set {key} <value>"))
                })?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(key),
                        plain(format!(" = {}", value.display())),
                    ])],
                    footer: None,
                })
            }
            Some("set") => {
                let key = positional_arg(parsed, "key")?;
                let raw = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| CmdError::new("E_CMD_ARG", "缺少参数：value"))?;
                let value = SettingValue::parse(raw);
                let known_themes = self.known_theme_ids();
                let known_sources: Vec<String> = self
                    .services
                    .online
                    .map(|online| {
                        online
                            .known_sources()
                            .into_iter()
                            .map(|source| source.as_str().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let known_sources: Vec<&str> = known_sources.iter().map(String::as_str).collect();
                validate_setting(key, &value, &known_themes, &known_sources)
                    .map_err(|message| CmdError::new("E_CMD_ARG", message))?;

                if key == "theme.id" {
                    let theme = self
                        .services
                        .theme
                        .ok_or_else(|| CmdError::new("E_CMD_ARG", "主题服务未接入"))?;
                    let id = value.as_str().unwrap_or_default();
                    let applied = theme.apply(id).map_err(normalize_cmd_dots)?;
                    settings.set_and_save(key, value.clone())?;
                    theme.broadcast(&applied.id);
                } else if let Some(source_id) = source_id_from_enabled_key(key) {
                    let online = self
                        .services
                        .online
                        .ok_or_else(|| CmdError::new("E_CMD_ARG", "在线音源服务未接入"))?;
                    let source = known_source_by_str(&known_source_ids(online), source_id)
                        .ok_or_else(|| {
                            CmdError::new("E_CMD_ARG", format!("未知音源：{source_id}"))
                        })?;
                    settings.set_and_save(key, value.clone())?;
                    if let Err(error) =
                        online.set_source_enabled(source, value.as_bool().unwrap_or(true))
                    {
                        let _ = settings.unset_and_save(key);
                        return Err(error);
                    }
                } else {
                    settings.set_and_save(key, value.clone())?;
                }

                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已写入"),
                        plain(format!(" · {key} = {}", value.display())),
                    ])],
                    footer: None,
                })
            }
            Some("unset") => {
                let key = positional_arg(parsed, "key")?;
                let removed = settings.unset_and_save(key)?;
                if removed.is_some() {
                    if key == "theme.id" {
                        if let Some(theme) = self.services.theme {
                            theme.broadcast("graphite");
                        }
                    } else if let Some(source_id) = source_id_from_enabled_key(key)
                        && let Some(online) = self.services.online
                        && let Some(source) =
                            known_source_by_str(&known_source_ids(online), source_id)
                    {
                        online.set_source_enabled(source, true)?;
                    }
                }
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(if removed.is_some() {
                            "已移除"
                        } else {
                            "未设置"
                        }),
                        plain(format!(" · {key}")),
                    ])],
                    footer: None,
                })
            }
            Some("reset") => {
                settings.reset_and_save()?;
                if let Some(online) = self.services.online {
                    for source in known_source_ids(online) {
                        online.set_source_enabled(source, true)?;
                    }
                }
                if let Some(theme) = self.services.theme {
                    theme.broadcast("graphite");
                }
                Ok(Outcome {
                    outputs: vec![text_of(vec![bold("已恢复默认设置")])],
                    footer: Some("settings.json 已清空".to_owned()),
                })
            }
            _ => {
                let entries = settings.entries();
                if entries.is_empty() {
                    return Ok(Outcome {
                        outputs: vec![text_of(vec![
                            bold("暂无设置"),
                            dim(" · 所有项使用内置默认"),
                        ])],
                        footer: Some(format!(
                            "文件：{}",
                            np_core::settings::settings_path(settings.dir()).display()
                        )),
                    });
                }
                let mut outputs: Vec<Output> = entries
                    .iter()
                    .map(|(key, value)| {
                        text_of(vec![plain(format!("{key} = ")), bold(value.display())])
                    })
                    .collect();
                for warning in settings.warnings() {
                    outputs.push(hint_output(&warning.message));
                }
                Ok(Outcome {
                    outputs,
                    footer: Some(format!(
                        "文件：{}",
                        np_core::settings::settings_path(settings.dir()).display()
                    )),
                })
            }
        }
    }

    fn known_theme_ids(&self) -> Vec<String> {
        self.services
            .theme
            .and_then(|theme| theme.list().ok())
            .map(|entries| entries.into_iter().map(|entry| entry.id).collect())
            .unwrap_or_default()
    }

    /// `source`：list / enable / disable；写入 settings 并立即影响搜索。
    fn source(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let settings = self
            .services
            .settings
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "设置服务未接入"))?;
        let online = self
            .services
            .online
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "在线音源服务未接入"))?;
        let known = online.known_sources();

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("enable") | Some("disable") => {
                let text = positional_arg(parsed, "音源 id")?;
                let source = known
                    .iter()
                    .copied()
                    .find(|source| source.as_str() == text)
                    .ok_or_else(|| {
                        CmdError::new("E_CMD_ARG", format!("未知音源：{text}")).with_hint(format!(
                            "可用：{}",
                            known
                                .iter()
                                .map(|source| source.as_str())
                                .collect::<Vec<_>>()
                                .join(" / ")
                        ))
                    })?;
                let enabled = parsed.subcommand.map(|subcommand| subcommand.name) == Some("enable");
                let key = format!("source.{}.enabled", source.as_str());
                let known_ids: Vec<&str> = known.iter().map(|source| source.as_str()).collect();
                validate_setting(&key, &SettingValue::Bool(enabled), &[], &known_ids)
                    .map_err(|message| CmdError::new("E_CMD_ARG", message))?;

                settings.set_and_save(&key, SettingValue::Bool(enabled))?;
                if let Err(error) = online.set_source_enabled(source, enabled) {
                    let _ = settings.unset_and_save(&key);
                    return Err(error);
                }

                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(if enabled { "已启用" } else { "已禁用" }),
                        plain(format!(" · {} ({})", source_label(source), source.as_str())),
                    ])],
                    footer: Some(if enabled {
                        "下一次搜索会包含该源".to_owned()
                    } else {
                        "搜索会跳过该源；已在播放/下载的曲目不受影响".to_owned()
                    }),
                })
            }
            _ => {
                let rows = known
                    .iter()
                    .map(|source| {
                        let enabled = settings
                            .get_bool(&format!("source.{}.enabled", source.as_str()))
                            .unwrap_or(true);
                        vec![
                            source.as_str().to_owned(),
                            source_label(*source).to_owned(),
                            if enabled {
                                "启用".to_owned()
                            } else {
                                "禁用".to_owned()
                            },
                            source_description(*source).to_owned(),
                        ]
                    })
                    .collect();
                Ok(Outcome {
                    outputs: vec![
                        text_of(vec![bold("在线音源")]),
                        Output::Table {
                            columns: vec![
                                "id".to_owned(),
                                "名称".to_owned(),
                                "状态".to_owned(),
                                "说明".to_owned(),
                            ],
                            rows,
                            selectable: false,
                        },
                    ],
                    footer: Some("source enable/disable <id>".to_owned()),
                })
            }
        }
    }

    /// `cache`：stat / clear；清 streams 前若正在播远程曲目先停播。
    fn cache(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let online = self
            .services
            .online
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "在线音源服务未接入"))?;

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("clear") => {
                let raw = parsed
                    .positional
                    .first()
                    .map(String::as_str)
                    .unwrap_or("all");
                let target = CacheTarget::parse(raw)?;
                let stopped = self.stop_remote_playback()?;
                let released = online.clear_cache(target)?;

                let mut spans = vec![
                    bold("缓存已清理"),
                    plain(format!(" · {}", target_label(target))),
                    dim(format!(" · 释放 {}", format_bytes(released))),
                ];
                if stopped {
                    spans.push(dim(" · 已先停止播放远程曲目"));
                }
                let outputs = vec![text_of(spans)];
                Ok(Outcome {
                    outputs,
                    footer: None,
                })
            }
            _ => {
                let usage = online.cache_usage()?;
                let mut outputs = vec![
                    text_of(vec![
                        bold("streams"),
                        plain(format!(
                            " · {} 个文件 / {}",
                            usage.streams_files,
                            format_bytes(usage.streams_bytes)
                        )),
                    ]),
                    text_of(vec![
                        bold("合计"),
                        plain(format!(
                            " · {} 个文件 / {}",
                            usage.total_files(),
                            format_bytes(usage.total_bytes())
                        )),
                    ]),
                ];
                if usage.total_files() == 0 {
                    outputs.push(hint_output("缓存为空；播放在线曲目时会自动创建"));
                }
                Ok(Outcome {
                    outputs,
                    footer: Some("cache clear [streams|all]".to_owned()),
                })
            }
        }
    }

    /// 正在播放的曲目若是远程缓存，先停止并把它从队列移除（保留其它队列项）。
    fn stop_remote_playback(&self) -> Result<bool, CmdError> {
        let state = self.api.snapshot()?;
        let Some(path) = state.track_id.as_deref() else {
            return Ok(false);
        };
        let is_remote = self
            .library
            .track_by_path(path)?
            .is_some_and(|track| track.kind == "remote");
        if !is_remote {
            return Ok(false);
        }

        let view = self.api.queue_list()?;
        if let Some(index) = view.index {
            self.api.queue_remove(index)?;
        } else {
            self.api.queue_clear()?;
        }
        Ok(true)
    }

    fn help(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let registry = registry();

        match parsed.positional.first() {
            Some(name) => {
                let Some(spec) = registry.find(name) else {
                    return Err(CmdError {
                        code: "E_CMD_UNKNOWN".to_owned(),
                        message: format!("未识别的命令：{name}"),
                        hint: registry.suggest(name).map(|spec| suggestion(spec.name)),
                    });
                };
                let (description, examples) = help_entry(spec.name);
                let mut outputs = vec![text_of(vec![
                    bold(spec.usage),
                    plain(format!("  {description}")),
                ])];
                if !spec.args.is_empty() {
                    outputs.push(text_of(vec![bold("参数")]));
                    for arg in spec.args {
                        let suffix = if arg.required { "" } else { "（可选）" };
                        outputs.push(text_of(vec![plain(format!("  <{}>{}", arg.label, suffix))]));
                    }
                }
                if !spec.flags.is_empty() {
                    outputs.push(text_of(vec![bold("选项")]));
                    for flag in spec.flags {
                        outputs.push(text_of(vec![
                            plain(format!("  --{}", flag.name)),
                            dim(format!("  {}", flag.summary)),
                        ]));
                    }
                }
                if !spec.subcommands.is_empty() {
                    outputs.push(text_of(vec![bold("子命令")]));
                    for subcommand in spec.subcommands {
                        outputs.push(text_of(vec![
                            plain(format!("  {}", subcommand.usage)),
                            dim(format!("  {}", subcommand.summary)),
                        ]));
                    }
                }
                if !examples.is_empty() {
                    outputs.push(text_of(vec![bold("例")]));
                    for example in examples {
                        outputs.push(text_of(vec![plain(format!("  {example}"))]));
                    }
                }
                Ok(Outcome {
                    outputs,
                    footer: Some("输入 help 查看分组列表".to_owned()),
                })
            }
            None => {
                let mut outputs = Vec::new();
                for (group, names) in HELP_GROUPS {
                    outputs.push(text_of(vec![bold(*group)]));
                    for name in *names {
                        if let Some(spec) = registry.find(name) {
                            outputs.push(text_of(vec![
                                plain(format!("  {:<10}", spec.name)),
                                dim(spec.summary),
                            ]));
                        }
                    }
                }
                Ok(Outcome {
                    outputs,
                    footer: Some("输入 help <命令> 查看参数与例子".to_owned()),
                })
            }
        }
    }

    /// `version`：执行 crate 的版本。
    fn version() -> Outcome {
        Outcome {
            outputs: vec![text_of(vec![
                bold("Nowhere Player"),
                plain(format!(" {}", env!("CARGO_PKG_VERSION"))),
            ])],
            footer: None,
        }
    }
}
