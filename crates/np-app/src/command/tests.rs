use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use np_core::command::parse;
use np_core::{PlaybackMode, PlaybackStatus, RepeatMode};
use np_sources::{DownloadOutcome, ResolvedStream};

use super::settings::KNOWN_SOURCES;
use super::*;

/// `FakeApi` 记录的一次调用。
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Snapshot,
    Play(Option<String>),
    Pause,
    Resume,
    Next,
    Prev,
    Seek(u64),
    Volume(f32),
    Mode(PlaybackMode),
    QueueList,
    QueueAdd(Vec<String>),
    QueueRemove(usize),
    QueueClear,
    QueueJump(usize),
}

/// 单测用播放器：记录调用、返回构造值，不碰真实设备。
struct FakeApi {
    calls: Mutex<Vec<Call>>,
    state: Mutex<PlaybackState>,
    queue: Mutex<QueueView>,
    reject_paths: Mutex<Vec<String>>,
    playback_error: Mutex<Option<CmdError>>,
}

impl FakeApi {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            state: Mutex::new(PlaybackState {
                volume: 0.5,
                ..PlaybackState::default()
            }),
            queue: Mutex::new(QueueView {
                tracks: Vec::new(),
                index: None,
            }),
            reject_paths: Mutex::new(Vec::new()),
            playback_error: Mutex::new(None),
        }
    }

    fn with_state(self, state: PlaybackState) -> Self {
        *self.state.lock().expect("state mutex") = state;
        self
    }

    fn with_queue(self, queue: QueueView) -> Self {
        *self.queue.lock().expect("queue mutex") = queue;
        self
    }

    fn with_playback_error(self, error: CmdError) -> Self {
        *self.playback_error.lock().expect("error mutex") = Some(error);
        self
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls mutex").clone()
    }

    fn record(&self, call: Call) {
        self.calls.lock().expect("calls mutex").push(call);
    }

    /// 播放类命令的公共返回：配置了错误就回错误，否则回当前快照。
    fn playback(&self) -> Result<PlaybackState, CmdError> {
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        Ok(self.state.lock().expect("state mutex").clone())
    }
}

impl PlayerApi for FakeApi {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        self.record(Call::Snapshot);
        self.playback()
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        self.record(Call::Play(path.clone()));
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        {
            let mut state = self.state.lock().expect("state mutex");
            if path.is_some() {
                state.track_id = path;
            }
            state.status = PlaybackStatus::Playing;
        }
        self.playback()
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        self.record(Call::Pause);
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        self.state.lock().expect("state mutex").status = PlaybackStatus::Paused;
        self.playback()
    }

    fn resume(&self) -> Result<PlaybackState, CmdError> {
        self.record(Call::Resume);
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        self.state.lock().expect("state mutex").status = PlaybackStatus::Playing;
        self.playback()
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.record(Call::Next);
        self.playback()
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.record(Call::Prev);
        self.playback()
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
        self.record(Call::Seek(position_ms));
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        self.state.lock().expect("state mutex").position_ms = position_ms;
        self.playback()
    }

    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
        self.record(Call::Volume(volume));
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        self.state.lock().expect("state mutex").volume = volume;
        self.playback()
    }

    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        self.record(Call::Mode(mode));
        if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
            return Err(error);
        }
        self.state.lock().expect("state mutex").mode = mode;
        self.playback()
    }

    fn queue_list(&self) -> Result<QueueView, CmdError> {
        self.record(Call::QueueList);
        Ok(self.queue.lock().expect("queue mutex").clone())
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
        self.record(Call::QueueAdd(paths.clone()));
        {
            let reject = self.reject_paths.lock().expect("reject mutex");
            if let Some(path) = paths.iter().find(|path| reject.contains(path)) {
                return Err(CmdError::new("E_DECODE", format!("无法加入：{path}")));
            }
        }
        let mut queue = self.queue.lock().expect("queue mutex");
        queue.tracks.extend(paths);
        if queue.index.is_none() {
            queue.index = Some(0);
        }
        Ok(queue.clone())
    }

    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
        self.record(Call::QueueRemove(index));
        let mut queue = self.queue.lock().expect("queue mutex");
        if index >= queue.tracks.len() {
            return Err(CmdError::new("E_DECODE", "队列下标越界"));
        }
        queue.tracks.remove(index);
        if queue.tracks.is_empty() {
            queue.index = None;
        } else if let Some(current) = queue.index {
            if index < current {
                queue.index = Some(current - 1);
            } else if index == current {
                queue.index = Some(current.min(queue.tracks.len() - 1));
            }
        }
        Ok(queue.clone())
    }

    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        self.record(Call::QueueClear);
        let mut queue = self.queue.lock().expect("queue mutex");
        queue.tracks.clear();
        queue.index = None;
        Ok(queue.clone())
    }

    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
        self.record(Call::QueueJump(index));
        let mut queue = self.queue.lock().expect("queue mutex");
        if index >= queue.tracks.len() {
            return Err(CmdError::new("E_DECODE", "队列下标越界"));
        }
        queue.index = Some(index);
        Ok(queue.clone())
    }
}

fn queue_view(tracks: &[&str], index: Option<usize>) -> QueueView {
    QueueView {
        tracks: tracks.iter().map(|track| (*track).to_owned()).collect(),
        index,
    }
}

/// 构造一首假曲目；只填命令层用得到的字段。
fn track(id: i64, title: &str, artist: &str, path: &str, duration_ms: i64) -> Track {
    Track {
        id,
        kind: "local".to_owned(),
        path: path.to_owned(),
        source: "local".to_owned(),
        source_id: String::new(),
        title: title.to_owned(),
        album_id: None,
        album: String::new(),
        artist: artist.to_owned(),
        album_artist: String::new(),
        track_no: 0,
        disc_no: 0,
        year: 0,
        duration_ms,
        sample_rate: 44_100,
        channels: 2,
        bitrate: 320,
        codec: "flac".to_owned(),
        size_bytes: 0,
        mtime_ms: 0,
        added_at: 0,
        last_seen_at: 0,
    }
}

/// `FakeLibraryApi` 记录的一次调用。
#[derive(Debug, Clone, PartialEq, Eq)]
enum LibraryCall {
    AddRoot(String),
    ListRoots,
    Scan(bool),
    Search(String, usize),
    ListRecent(usize),
    Stats,
}

struct FakeLibraryState {
    roots: Vec<String>,
    list_tracks: Vec<Track>,
    search_tracks: Vec<Track>,
    stats: Stats,
    busy: bool,
    session: Vec<SearchHit>,
    active_listing: Option<ActiveListing>,
    library_listing: Vec<Track>,
    tracks_by_path: std::collections::HashMap<String, Track>,
    playlists: Vec<Playlist>,
    playlist_items: HashMap<i64, Vec<PlaylistItem>>,
    next_playlist_id: i64,
    next_item_id: i64,
    calls: Vec<LibraryCall>,
}

/// 单测用媒体库：不碰文件系统，行为完全由构造值决定。
struct FakeLibraryApi {
    state: Mutex<FakeLibraryState>,
}

impl FakeLibraryApi {
    fn new() -> Self {
        Self {
            state: Mutex::new(FakeLibraryState {
                roots: Vec::new(),
                list_tracks: Vec::new(),
                search_tracks: Vec::new(),
                stats: Stats::default(),
                busy: false,
                session: Vec::new(),
                active_listing: None,
                library_listing: Vec::new(),
                tracks_by_path: std::collections::HashMap::new(),
                playlists: Vec::new(),
                playlist_items: HashMap::new(),
                next_playlist_id: 1,
                next_item_id: 1,
                calls: Vec::new(),
            }),
        }
    }

    fn with_roots(self, roots: &[&str]) -> Self {
        self.state.lock().expect("state mutex").roots =
            roots.iter().map(|root| (*root).to_owned()).collect();
        self
    }

    fn with_list_tracks(self, tracks: Vec<Track>) -> Self {
        self.state.lock().expect("state mutex").list_tracks = tracks;
        self
    }

    fn with_search_tracks(self, tracks: Vec<Track>) -> Self {
        self.state.lock().expect("state mutex").search_tracks = tracks;
        self
    }

    fn with_track_by_path(self, path: &str, track: Track) -> Self {
        self.state
            .lock()
            .expect("state mutex")
            .tracks_by_path
            .insert(path.to_owned(), track);
        self
    }

    fn with_stats(self, stats: Stats) -> Self {
        self.state.lock().expect("state mutex").stats = stats;
        self
    }

    fn busy(self) -> Self {
        self.state.lock().expect("state mutex").busy = true;
        self
    }

    fn calls(&self) -> Vec<LibraryCall> {
        self.state.lock().expect("state mutex").calls.clone()
    }

    fn session(&self) -> Vec<Track> {
        self.state
            .lock()
            .expect("state mutex")
            .session
            .iter()
            .filter_map(|hit| match hit {
                SearchHit::Local(track) => Some(track.clone()),
                SearchHit::Online(_) => None,
            })
            .collect()
    }

    fn hits(&self) -> Vec<SearchHit> {
        self.state.lock().expect("state mutex").session.clone()
    }

    fn record(&self, call: LibraryCall) {
        self.state.lock().expect("state mutex").calls.push(call);
    }
}

impl LibraryApi for FakeLibraryApi {
    fn add_root(&self, path: &str) -> Result<usize, CmdError> {
        self.record(LibraryCall::AddRoot(path.to_owned()));
        let mut state = self.state.lock().expect("state mutex");
        if state.roots.iter().any(|root| root == path) {
            return Err(CmdError::new("E_CMD_ARG", format!("扫描根已存在：{path}")));
        }
        state.roots.push(path.to_owned());
        Ok(state.roots.len())
    }

    fn list_roots(&self) -> Result<Vec<String>, CmdError> {
        self.record(LibraryCall::ListRoots);
        Ok(self.state.lock().expect("state mutex").roots.clone())
    }

    fn scan(&self, full: bool, _observer: Arc<dyn LibraryObserver>) -> Result<(), CmdError> {
        self.record(LibraryCall::Scan(full));
        let state = self.state.lock().expect("state mutex");
        if state.busy {
            return Err(CmdError::new("E_BUSY", "已有扫描在进行中"));
        }
        if state.roots.is_empty() {
            return Err(CmdError::new("E_CMD_ARG", "还没有扫描根"));
        }
        Ok(())
    }

    fn search(&self, keyword: &str, limit: usize) -> Result<Vec<Track>, CmdError> {
        self.record(LibraryCall::Search(keyword.to_owned(), limit));
        let state = self.state.lock().expect("state mutex");
        Ok(state.search_tracks.iter().take(limit).cloned().collect())
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Track>, CmdError> {
        self.record(LibraryCall::ListRecent(limit));
        let state = self.state.lock().expect("state mutex");
        Ok(state.list_tracks.iter().take(limit).cloned().collect())
    }

    fn stats(&self) -> Result<Stats, CmdError> {
        self.record(LibraryCall::Stats);
        Ok(self.state.lock().expect("state mutex").stats)
    }

    fn remember_search(&self, tracks: Vec<Track>) {
        self.state.lock().expect("state mutex").session =
            tracks.into_iter().map(SearchHit::Local).collect();
    }

    fn search_results(&self) -> Vec<Track> {
        self.state
            .lock()
            .expect("state mutex")
            .session
            .iter()
            .filter_map(|hit| match hit {
                SearchHit::Local(track) => Some(track.clone()),
                SearchHit::Online(_) => None,
            })
            .collect()
    }

    fn remember_hits(&self, hits: Vec<SearchHit>) {
        self.state.lock().expect("state mutex").session = hits;
    }

    fn search_hits(&self) -> Vec<SearchHit> {
        self.state.lock().expect("state mutex").session.clone()
    }

    fn set_active_listing(&self, listing: ActiveListing) {
        self.state.lock().expect("state mutex").active_listing = Some(listing);
    }

    fn active_listing(&self) -> Option<ActiveListing> {
        self.state
            .lock()
            .expect("state mutex")
            .active_listing
            .clone()
    }

    fn remember_library_listing(&self, tracks: Vec<Track>) {
        self.state.lock().expect("state mutex").library_listing = tracks;
    }

    fn library_listing(&self) -> Vec<Track> {
        self.state
            .lock()
            .expect("state mutex")
            .library_listing
            .clone()
    }

    fn track_by_path(&self, path: &str) -> Result<Option<Track>, CmdError> {
        Ok(self
            .state
            .lock()
            .expect("state mutex")
            .tracks_by_path
            .get(path)
            .cloned())
    }

    fn playlist_create(&self, name: &str) -> Result<i64, CmdError> {
        let name = np_library::validate_playlist_name(name)
            .map_err(|error| CmdError::new("E_CMD_ARG", error.to_string()))?;
        let mut state = self.state.lock().expect("state mutex");
        if state
            .playlists
            .iter()
            .any(|playlist| playlist.name.eq_ignore_ascii_case(&name))
        {
            return Err(CmdError::new("E_CMD_ARG", format!("歌单已存在：{name}")));
        }
        let id = state.next_playlist_id;
        state.next_playlist_id += 1;
        state.playlists.push(Playlist {
            id,
            name,
            created_at: 0,
            updated_at: 0,
        });
        state.playlist_items.insert(id, Vec::new());
        Ok(id)
    }

    fn playlist_rename(&self, id: i64, name: &str) -> Result<(), CmdError> {
        let name = np_library::validate_playlist_name(name)
            .map_err(|error| CmdError::new("E_CMD_ARG", error.to_string()))?;
        let mut state = self.state.lock().expect("state mutex");
        if state
            .playlists
            .iter()
            .any(|playlist| playlist.id != id && playlist.name.eq_ignore_ascii_case(&name))
        {
            return Err(CmdError::new("E_CMD_ARG", format!("歌单已存在：{name}")));
        }
        let playlist = state
            .playlists
            .iter_mut()
            .find(|playlist| playlist.id == id)
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "歌单不存在"))?;
        playlist.name = name;
        Ok(())
    }

    fn playlist_delete(&self, id: i64) -> Result<(), CmdError> {
        let mut state = self.state.lock().expect("state mutex");
        let before = state.playlists.len();
        state.playlists.retain(|playlist| playlist.id != id);
        if state.playlists.len() == before {
            return Err(CmdError::new("E_CMD_ARG", "歌单不存在"));
        }
        state.playlist_items.remove(&id);
        Ok(())
    }

    fn playlist_list(&self) -> Result<Vec<PlaylistSummary>, CmdError> {
        let state = self.state.lock().expect("state mutex");
        Ok(state
            .playlists
            .iter()
            .map(|playlist| PlaylistSummary {
                id: playlist.id,
                name: playlist.name.clone(),
                item_count: state
                    .playlist_items
                    .get(&playlist.id)
                    .map_or(0, |items| items.len() as u64),
                created_at: playlist.created_at,
                updated_at: playlist.updated_at,
            })
            .collect())
    }

    fn playlist_by_name(&self, name: &str) -> Result<Option<Playlist>, CmdError> {
        let state = self.state.lock().expect("state mutex");
        Ok(state
            .playlists
            .iter()
            .find(|playlist| playlist.name.eq_ignore_ascii_case(name))
            .cloned())
    }

    fn playlist_get(&self, id: i64) -> Result<(Playlist, Vec<PlaylistItem>), CmdError> {
        let state = self.state.lock().expect("state mutex");
        let playlist = state
            .playlists
            .iter()
            .find(|playlist| playlist.id == id)
            .cloned()
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "歌单不存在"))?;
        let items = state.playlist_items.get(&id).cloned().unwrap_or_default();
        Ok((playlist, items))
    }

    fn playlist_add(&self, id: i64, item: &NewPlaylistItem) -> Result<PlaylistItem, CmdError> {
        let mut state = self.state.lock().expect("state mutex");
        if !state.playlists.iter().any(|playlist| playlist.id == id) {
            return Err(CmdError::new("E_CMD_ARG", "歌单不存在"));
        }
        let item_id = state.next_item_id;
        state.next_item_id += 1;
        let items = state.playlist_items.entry(id).or_default();
        let position = items.len() as i64;
        let added = PlaylistItem {
            id: item_id,
            playlist_id: id,
            position,
            kind: item.kind.clone(),
            path: item.path.clone(),
            source: item.source.clone(),
            source_id: item.source_id.clone(),
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            duration_ms: item.duration_ms,
            added_at: 0,
        };
        items.push(added.clone());
        Ok(added)
    }

    fn playlist_remove(&self, id: i64, position: i64) -> Result<(), CmdError> {
        let mut state = self.state.lock().expect("state mutex");
        let items = state
            .playlist_items
            .get_mut(&id)
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "歌单不存在"))?;
        if position < 0 || position as usize >= items.len() {
            return Err(CmdError::new(
                "E_CMD_ARG",
                format!("歌单里没有第 {} 首", position + 1),
            ));
        }
        items.remove(position as usize);
        for (index, item) in items.iter_mut().enumerate() {
            item.position = index as i64;
        }
        Ok(())
    }
}

#[test]
fn fake_api_records_calls() {
    let api = FakeApi::new();
    api.pause().expect("pause succeeds");
    api.seek(1_500).expect("seek succeeds");

    assert_eq!(api.calls(), vec![Call::Pause, Call::Seek(1_500)]);
}

#[test]
fn parse_error_keeps_code_and_usage_hint() {
    let error = parse("plya").expect_err("unknown command must fail");
    let error = CmdError::from_parse(&error);

    assert_eq!(error.code, "E_CMD_UNKNOWN");
    assert_eq!(error.hint.as_deref(), Some("did you mean 'play'?"));
}

#[test]
fn player_error_maps_to_ipc_code() {
    let error = CmdError::from_player(PlayerError::unsupported("没有可暂停的曲目"));

    assert_eq!(error.code, "E_UNSUPPORTED");
    assert_eq!(error.message, "没有可暂停的曲目");
    assert_eq!(error.hint, None);
}

#[test]
fn usage_hint_is_filled_when_missing() {
    let parsed = parse("vol 5")
        .expect("valid input")
        .expect("non-empty input");
    let error = CmdError::new("E_DECODE", "设备丢失").with_usage(&parsed);

    assert_eq!(error.hint.as_deref(), Some("用法：vol <音量>"));
}

#[test]
fn repeat_mode_defaults_to_off_for_unknown_values() {
    // 解析层已把枚举归一化；这里只锁住“未知值绝不 panic”。
    let mode = PlaybackMode {
        repeat: RepeatMode::Off,
        shuffle: false,
    };
    assert_eq!(mode, PlaybackMode::default());
}

// ---- 执行器：命令映射 ----

fn done_block(api: &FakeApi, input: &str) -> Block {
    match execute(api, "block-1", input) {
        Execution::Blocks { done, .. } => *done,
        Execution::Accepted { .. } => panic!("unexpected accepted block for input: {input}"),
        Execution::Ignored => panic!("expected a block for input: {input}"),
    }
}

fn text_of_block(block: &Block) -> String {
    block
        .outputs
        .iter()
        .filter_map(|output| match output {
            Output::Text { spans, .. } => Some(
                spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn error_fields(block: &Block) -> (&str, &str, Option<&str>) {
    block
        .outputs
        .iter()
        .find_map(|output| match output {
            Output::Error {
                code,
                message,
                hint,
                ..
            } => Some((code.as_str(), message.as_str(), hint.as_deref())),
            _ => None,
        })
        .expect("block must contain an error output")
}

#[test]
fn play_without_argument_resumes() {
    let api = FakeApi::new();
    let block = done_block(&api, "play");

    assert_eq!(api.calls(), vec![Call::Resume]);
    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("继续播放"));
}

#[test]
fn play_number_is_one_based_and_jumps_after_queue_list() {
    let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], None));
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, "queue list");
    let block = done_with_library(&api, &library, "play 3");

    assert_eq!(
        api.calls(),
        vec![Call::QueueList, Call::QueueList, Call::QueueJump(2)]
    );
    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("第 3 首"));
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：播放队列"))
    );
}

#[test]
fn play_zero_is_rejected_before_any_call() {
    let api = FakeApi::new();
    let block = done_block(&api, "play 0");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(api.calls().is_empty());
}

#[test]
fn play_path_is_passed_through() {
    let api = FakeApi::new();
    let block = done_block(&api, r"play C:\Music\a.flac");

    assert_eq!(
        api.calls(),
        vec![Call::Play(Some(r"C:\Music\a.flac".to_owned()))]
    );
    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("a.flac"));
}

#[test]
fn pause_resume_next_prev_map_to_same_named_calls() {
    let api = FakeApi::new();

    for input in ["pause", "resume", "next", "prev"] {
        assert_eq!(done_block(&api, input).status, BlockStatus::Done);
    }

    assert_eq!(
        api.calls(),
        vec![Call::Pause, Call::Resume, Call::Next, Call::Prev]
    );
}

#[test]
fn vol_absolute_sets_level_and_clamps_high() {
    let api = FakeApi::new();
    let block = done_block(&api, "vol 150");

    assert_eq!(api.calls(), vec![Call::Volume(1.0)]);
    assert!(text_of_block(&block).contains("100%"));
}

#[test]
fn vol_absolute_zero_is_allowed() {
    let api = FakeApi::new();
    let block = done_block(&api, "vol 0");

    assert_eq!(api.calls(), vec![Call::Volume(0.0)]);
    assert!(text_of_block(&block).contains("0%"));
}

#[test]
fn vol_relative_reads_current_level_first() {
    let api = FakeApi::new().with_state(PlaybackState {
        volume: 0.42,
        ..PlaybackState::default()
    });
    let block = done_block(&api, "vol +8");

    let calls = api.calls();
    assert_eq!(calls.first(), Some(&Call::Snapshot));
    match calls.get(1) {
        Some(Call::Volume(volume)) => assert!((volume - 0.5).abs() < 1e-6),
        other => panic!("expected volume call, got {other:?}"),
    }
    assert!(text_of_block(&block).contains("50%"));
}

#[test]
fn vol_relative_clamps_low() {
    let api = FakeApi::new().with_state(PlaybackState {
        volume: 0.5,
        ..PlaybackState::default()
    });
    let block = done_block(&api, "vol -200");

    assert_eq!(api.calls(), vec![Call::Snapshot, Call::Volume(0.0)]);
    assert!(text_of_block(&block).contains("0%"));
}

#[test]
fn seek_plain_number_is_milliseconds() {
    let api = FakeApi::new();
    done_block(&api, "seek 90000");

    assert_eq!(api.calls(), vec![Call::Seek(90_000)]);
}

#[test]
fn seek_mm_ss_is_absolute() {
    let api = FakeApi::new();
    done_block(&api, "seek 1:30");

    assert_eq!(api.calls(), vec![Call::Seek(90_000)]);
}

#[test]
fn seek_relative_seconds_uses_current_position() {
    let api = FakeApi::new().with_state(PlaybackState {
        position_ms: 30_000,
        duration_ms: Some(180_000),
        ..PlaybackState::default()
    });
    done_block(&api, "seek +15");

    assert_eq!(api.calls(), vec![Call::Snapshot, Call::Seek(45_000)]);
}

#[test]
fn seek_relative_clamps_to_track_bounds() {
    let api = FakeApi::new().with_state(PlaybackState {
        position_ms: 30_000,
        duration_ms: Some(180_000),
        ..PlaybackState::default()
    });

    done_block(&api, "seek -40");
    done_block(&api, "seek +999");

    assert_eq!(
        api.calls(),
        vec![
            Call::Snapshot,
            Call::Seek(0),
            Call::Snapshot,
            Call::Seek(180_000)
        ]
    );
}

#[test]
fn mode_repeat_sends_complete_mode_once() {
    let api = FakeApi::new().with_state(PlaybackState {
        mode: PlaybackMode {
            repeat: RepeatMode::Off,
            shuffle: true,
        },
        ..PlaybackState::default()
    });
    let block = done_block(&api, "mode repeat all");

    let calls = api.calls();
    assert_eq!(calls.first(), Some(&Call::Snapshot));
    let modes: Vec<&Call> = calls
        .iter()
        .filter(|call| matches!(call, Call::Mode(_)))
        .collect();
    assert_eq!(modes.len(), 1, "mode 必须一次下发完整对象");
    assert_eq!(
        modes[0],
        &Call::Mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: true,
        })
    );
    assert!(text_of_block(&block).contains("循环：列表"));
}

#[test]
fn mode_shuffle_keeps_repeat_from_snapshot() {
    let api = FakeApi::new().with_state(PlaybackState {
        mode: PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: false,
        },
        ..PlaybackState::default()
    });
    let block = done_block(&api, "mode shuffle on");

    assert_eq!(
        api.calls(),
        vec![
            Call::Snapshot,
            Call::Mode(PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: true,
            })
        ]
    );
    assert!(text_of_block(&block).contains("随机：开"));
}

#[test]
fn queue_list_shows_local_metadata_in_four_columns() {
    let api = FakeApi::new().with_queue(queue_view(&["/music/a.flac", "/music/b.flac"], Some(1)));
    let library = FakeLibraryApi::new()
        .with_track_by_path(
            "/music/a.flac",
            track(1, "第一首", "甲", "/music/a.flac", 65_000),
        )
        .with_track_by_path(
            "/music/b.flac",
            track(2, "第二首", "乙", "/music/b.flac", 125_000),
        );
    let block = done_with_library(&api, &library, "queue list");

    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns, ["序号", "标题", "艺人", "时长"]);
    assert_eq!(rows[0], ["1", "第一首", "甲", "01:05"]);
    assert_eq!(rows[1], ["▶ 2", "第二首", "乙", "02:05"]);
}

#[test]
fn queue_list_shows_remote_source_column() {
    let api = FakeApi::new().with_queue(queue_view(&["/cache/bili.m4a"], Some(0)));
    let mut remote = track(0, "远程歌", "上传者", "/cache/bili.m4a", 200_000);
    remote.kind = "remote".to_owned();
    remote.source = "bilibili".to_owned();
    let library = FakeLibraryApi::new().with_track_by_path("/cache/bili.m4a", remote);
    let block = done_with_library(&api, &library, "queue list");

    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns, ["序号", "标题", "艺人", "时长", "来源"]);
    assert_eq!(rows[0], ["▶ 1", "远程歌", "上传者", "03:20", "B站"]);
}

#[test]
fn queue_list_falls_back_to_filename_and_dashes() {
    let api = FakeApi::new().with_queue(queue_view(&["/music/missing.flac"], None));
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "queue list");

    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns.len(), 4);
    assert_eq!(rows[0], ["1", "missing.flac", "—", "—"]);
}

#[test]
fn queue_list_mixed_queue_uses_source_column_with_local_dash() {
    let api = FakeApi::new().with_queue(queue_view(&["/music/a.flac", "/cache/net.m4a"], None));
    let mut remote = track(0, "云歌", "云艺人", "/cache/net.m4a", 90_000);
    remote.kind = "remote".to_owned();
    remote.source = "netease".to_owned();
    let library = FakeLibraryApi::new()
        .with_track_by_path(
            "/music/a.flac",
            track(1, "本地歌", "本地艺人", "/music/a.flac", 60_000),
        )
        .with_track_by_path("/cache/net.m4a", remote);
    let block = done_with_library(&api, &library, "queue list");

    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns.last().copied(), Some("来源"));
    assert_eq!(rows[0][4], "—");
    assert_eq!(rows[1][4], "网易云");
}

#[test]
fn queue_list_empty_still_has_metadata_columns() {
    let api = FakeApi::new().with_queue(queue_view(&[], None));
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "queue list");

    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns.len(), 4);
    assert!(rows.is_empty());
}

#[test]
fn queue_add_accepts_existing_file() {
    let dir = TestDir::new("queue-add-file");
    let path = dir.path().join("a b.flac");
    fs::write(&path, b"fake").expect("write");
    let input = format!("queue add \"{}\"", path.display());
    let api = FakeApi::new();
    let block = done_block(&api, &input);

    assert_eq!(
        api.calls(),
        vec![Call::QueueAdd(vec![path.to_string_lossy().into_owned()])]
    );
    assert!(text_of_block(&block).contains("已加入 1 首"));
    assert_eq!(block.footer.as_deref(), Some("队列现有 1 首"));
    assert!(!block.footer.as_deref().unwrap().contains("仍指向"));
}

#[test]
fn queue_add_footer_names_the_previous_listing() {
    let dir = TestDir::new("queue-add-footer");
    let path = dir.path().join("song.flac");
    fs::write(&path, b"fake").expect("write");
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Song",
        "Artist",
        "/music/song.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "search artist");
    let input = format!("queue add \"{}\"", path.display());
    let block = done_with_library(&api, &library, &input);

    assert_eq!(block.status, BlockStatus::Done);
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("仍指向搜索结果"))
    );
    assert!(
        library
            .active_listing()
            .is_some_and(|listing| listing.source == ListingSource::Search)
    );
}

#[test]
fn queue_add_missing_path_reports_file_not_found() {
    let api = FakeApi::new();
    let block = done_block(&api, "queue add 1");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert_eq!(error_fields(&block).1, "文件不存在：1");
    assert!(api.calls().is_empty());
}

#[test]
fn queue_add_requires_a_path_or_from_search() {
    let api = FakeApi::new();
    let block = done_block(&api, "queue add");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert_eq!(error_fields(&block).1, "缺少参数：路径");
    assert!(api.calls().is_empty());
}

#[test]
fn queue_add_from_search_adds_local_hit() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![
        track(1, "First", "Artist", "/music/first.flac", 1_000),
        track(2, "Second", "Artist", "/music/second.flac", 1_000),
    ]);
    done_with_library(&api, &library, "search artist");
    let block = done_with_library(&api, &library, "queue add --from-search 2");

    assert_eq!(
        api.calls(),
        vec![Call::QueueAdd(vec!["/music/second.flac".to_owned()])]
    );
    assert!(text_of_block(&block).contains("已加入 1 首"));
}

#[test]
fn queue_add_rejects_path_and_from_search_together() {
    let dir = TestDir::new("queue-add-both");
    let path = dir.path().join("a.flac");
    fs::write(&path, b"fake").expect("write");
    let input = format!("queue add \"{}\" --from-search 1", path.display());
    let api = FakeApi::new();
    let block = done_block(&api, &input);

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(api.calls().is_empty());
}

#[test]
fn queue_add_from_search_out_of_range_fails() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Only",
        "Artist",
        "/music/only.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "search only");
    let block = done_with_library(&api, &library, "queue add --from-search 2");

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("搜索结果只有 1 条"));
    assert!(api.calls().is_empty());
}

#[test]
fn queue_add_from_search_online_prepares_then_queues_local_path() {
    let dir = TestDir::new("queue-add-online");
    let cached = dir.path().join("cached.m4a");
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    library.remember_hits(vec![SearchHit::Online(online_track(
        SourceId::Bilibili,
        "BV1xx",
        "在线",
    ))]);
    library.set_active_listing(ActiveListing {
        source: ListingSource::Search,
        len: 1,
    });
    let online = FakeQueueOnline {
        path: cached.clone(),
    };
    let services = Services {
        settings: None,
        theme: None,
        online: Some(&online),
    };

    let block = match execute_with_services(
        &api,
        &library,
        services,
        "queue-add-online-block",
        "queue add --from-search 1",
        Arc::new(NoopLibraryObserver),
    ) {
        Execution::Blocks { done, .. } => *done,
        _ => panic!("expected blocks"),
    };

    assert_eq!(
        api.calls(),
        vec![Call::QueueAdd(vec![cached.to_string_lossy().into_owned()])]
    );
    assert!(text_of_block(&block).contains("已加入 1 首"));
}

#[test]
fn queue_add_from_search_without_number_fails() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "queue add --from-search");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(api.calls().is_empty());
}

#[test]
fn queue_add_success_does_not_replace_active_search_listing() {
    let api = FakeApi::new().with_queue(queue_view(&["queued.flac"], None));
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Search",
        "Artist",
        "/music/search.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "search song");
    done_with_library(&api, &library, "queue add --from-search 1");
    let block = done_with_library(&api, &library, "play 1");

    assert_eq!(
        api.calls(),
        vec![
            Call::QueueAdd(vec!["/music/search.flac".to_owned()]),
            Call::Play(Some("/music/search.flac".to_owned()))
        ]
    );
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：搜索结果"))
    );
}

#[test]
fn queue_remove_uses_one_based_index() {
    let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(0)));
    let block = done_block(&api, "queue remove 2");

    assert_eq!(api.calls(), vec![Call::QueueRemove(1)]);
    assert!(text_of_block(&block).contains("第 2 首"));
    assert_eq!(block.footer.as_deref(), Some("队列还剩 2 首"));
}

#[test]
fn queue_jump_uses_one_based_index() {
    let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(0)));
    let block = done_block(&api, "queue jump 3");

    assert_eq!(api.calls(), vec![Call::QueueJump(2)]);
    assert!(text_of_block(&block).contains("第 3 首"));
}

#[test]
fn queue_clear_empties_the_queue() {
    let api = FakeApi::new().with_queue(queue_view(&["a.flac"], Some(0)));
    let block = done_block(&api, "queue clear");

    assert_eq!(api.calls(), vec![Call::QueueClear]);
    assert!(text_of_block(&block).contains("清空"));
}

#[test]
fn parse_error_becomes_error_block_with_hint() {
    let api = FakeApi::new();
    let block = done_block(&api, "plya");

    assert_eq!(block.status, BlockStatus::Error);
    let (code, _, hint) = error_fields(&block);
    assert_eq!(code, "E_CMD_UNKNOWN");
    assert_eq!(hint, Some("did you mean 'play'?"));
    assert!(api.calls().is_empty());
}

#[test]
fn argument_error_carries_usage_hint() {
    let api = FakeApi::new();
    let block = done_block(&api, "seek 1:2:3");

    let (code, _, hint) = error_fields(&block);
    assert_eq!(code, "E_CMD_ARG");
    let hint = hint.expect("hint must not be empty");
    assert!(
        hint.contains("用法：seek <位置>"),
        "unexpected hint: {hint}"
    );
}

#[test]
fn unclosed_quote_error_still_has_a_hint() {
    let api = FakeApi::new();
    let block = done_block(&api, "play \"unterminated");

    let (code, _, hint) = error_fields(&block);
    assert_eq!(code, "E_CMD_ARG");
    assert!(hint.is_some_and(|hint| !hint.is_empty()));
}

#[test]
fn api_error_becomes_error_block_with_usage_hint() {
    let api =
        FakeApi::new().with_playback_error(CmdError::new("E_UNSUPPORTED", "没有可暂停的曲目"));
    let block = done_block(&api, "pause");

    assert_eq!(block.status, BlockStatus::Error);
    let (code, message, hint) = error_fields(&block);
    assert_eq!(code, "E_UNSUPPORTED");
    assert_eq!(message, "没有可暂停的曲目");
    assert_eq!(hint, Some("用法：pause"));
}

#[test]
fn pause_uses_library_title_not_cache_filename() {
    let api = FakeApi::new().with_state(PlaybackState {
        track_id: Some("/cache/bilibili-BV1pK411B7qx.m4a".to_owned()),
        position_ms: 108_000,
        duration_ms: Some(197_000),
        queue_length: Some(2),
        ..PlaybackState::default()
    });
    let mut remote = track(
        0,
        "【4K修复】周杰伦 - 晴天MV",
        "周杰伦",
        "/cache/bilibili-BV1pK411B7qx.m4a",
        197_000,
    );
    remote.kind = "remote".to_owned();
    remote.source = "bilibili".to_owned();
    remote.source_id = "BV1pK411B7qx".to_owned();
    let library =
        FakeLibraryApi::new().with_track_by_path("/cache/bilibili-BV1pK411B7qx.m4a", remote);

    let block = done_with_library(&api, &library, "pause");
    let text = text_of_block(&block);
    assert!(
        text.contains("【4K修复】周杰伦 - 晴天MV · 周杰伦"),
        "{text}"
    );
    assert!(!text.contains("bilibili-BV1pK411B7qx.m4a"), "{text}");
}

#[test]
fn play_path_and_seek_use_library_title() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_track_by_path(
        "/music/a.flac",
        track(1, "本地歌名", "本地艺人", "/music/a.flac", 65_000),
    );

    let play = done_with_library(&api, &library, "play /music/a.flac");
    let text = text_of_block(&play);
    assert!(text.contains("本地歌名 · 本地艺人"), "{text}");
    assert!(!text.contains("a.flac"), "{text}");

    let seek = done_with_library(&api, &library, "seek 1:30");
    assert!(text_of_block(&seek).contains("本地歌名 · 本地艺人"));
}

#[test]
fn queue_jump_uses_library_title() {
    let api = FakeApi::new().with_queue(queue_view(&["/music/q.flac"], Some(1)));
    let library = FakeLibraryApi::new().with_track_by_path(
        "/music/q.flac",
        track(1, "队列歌名", "队列艺人", "/music/q.flac", 12_000),
    );

    let block = done_with_library(&api, &library, "queue jump 1");
    let text = text_of_block(&block);
    assert!(text.contains("队列歌名 · 队列艺人"), "{text}");
    assert!(!text.contains("q.flac"), "{text}");
}

#[test]
fn help_lists_every_registered_command() {
    let api = FakeApi::new();
    let block = done_block(&api, "help");
    let text = text_of_block(&block);

    for spec in registry().specs() {
        assert!(text.contains(spec.name), "help 缺少命令：{}", spec.name);
    }
}

#[test]
fn help_with_argument_shows_usage() {
    let api = FakeApi::new();
    let block = done_block(&api, "help seek");

    assert!(text_of_block(&block).contains("seek <位置>"));
}

#[test]
fn help_groups_are_ordered_and_complete() {
    let api = FakeApi::new();
    let block = done_block(&api, "help");
    let text = text_of_block(&block);

    let positions: Vec<usize> = ["播放", "媒体库", "歌单", "系统"]
        .iter()
        .map(|group| text.find(group).expect("group missing"))
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    for spec in registry().specs() {
        assert!(text.contains(spec.name), "help 缺少 {}", spec.name);
    }
}

#[test]
fn help_play_has_description_and_example() {
    let api = FakeApi::new();
    let block = done_block(&api, "help play");
    let text = text_of_block(&block);

    assert!(text.contains("播放当前列表的第 n 首"));
    assert!(text.contains("play 3"));
    assert!(text.contains("play [<序号>|<路径>]"));
}

#[test]
fn search_registry_usage_lists_all_public_options() {
    let usage = registry().find("search").expect("search").usage;
    for token in [
        "关键词",
        "--artist",
        "--source",
        "--limit",
        "--local",
        "--up",
        "--mid",
    ] {
        assert!(usage.contains(token), "usage missing {token}: {usage}");
    }
}

#[test]
fn help_search_has_full_usage_flags_and_examples() {
    let api = FakeApi::new();
    let block = done_block(&api, "help search");
    let text = text_of_block(&block);

    assert!(text.contains("--artist"));
    assert!(text.contains("--source"));
    assert!(text.contains("--limit"));
    assert!(text.contains("--local"));
    assert!(text.contains("--up"));
    assert!(text.contains("search 晴天"));
    assert!(text.contains("search 最长的电影 --up 明前奶绿"));
}

#[test]
fn help_playlist_lists_all_subcommands() {
    let api = FakeApi::new();
    let block = done_block(&api, "help playlist");
    let text = text_of_block(&block);

    assert!(text.contains("playlist create"));
    assert!(text.contains("playlist add"));
    assert!(text.contains("playlist play"));
    assert!(text.contains("playlist remove"));
}

#[test]
fn version_reports_crate_version() {
    let api = FakeApi::new();
    let block = done_block(&api, "version");

    assert!(text_of_block(&block).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn clear_is_rejected_with_explanation() {
    let api = FakeApi::new();
    let block = done_block(&api, "clear");

    let (code, message, hint) = error_fields(&block);
    assert_eq!(code, "E_CMD_ARG");
    assert!(message.contains("clear 由终端本地处理"));
    assert!(hint.is_some_and(|hint| !hint.is_empty()));
}

#[test]
fn blank_input_produces_no_block() {
    let api = FakeApi::new();

    assert!(matches!(
        execute(&api, "block-1", "   "),
        Execution::Ignored
    ));
}

#[test]
fn block_is_running_then_done_with_the_same_id() {
    let api = FakeApi::new();
    let Execution::Blocks { running, done } = execute(&api, "id-1", "pause") else {
        panic!("expected blocks");
    };

    assert_eq!(running.id, "id-1");
    assert_eq!(running.status, BlockStatus::Running);
    assert!(running.outputs.is_empty());
    assert_eq!(running.duration_ms, None);

    assert_eq!(done.id, running.id);
    assert_eq!(done.input, "pause");
    assert_eq!(done.status, BlockStatus::Done);
    assert!(done.duration_ms.is_some());
    assert!(done.started_at > 0);
    assert_eq!(done.schema_version, SCHEMA_VERSION);
}

// ---- 媒体库命令（T-0023） ----

fn done_with_playlist_online(
    api: &FakeApi,
    library: &FakeLibraryApi,
    online: &dyn OnlineApi,
    input: &str,
) -> Block {
    let services = Services {
        settings: None,
        theme: None,
        online: Some(online),
    };
    match execute_with_services(
        api,
        library,
        services,
        "block-playlist-online",
        input,
        Arc::new(NoopLibraryObserver),
    ) {
        Execution::Blocks { done, .. } => *done,
        _ => panic!("expected a finished block for {input}"),
    }
}

fn done_with_library(api: &FakeApi, library: &FakeLibraryApi, input: &str) -> Block {
    match execute_with_library(
        api,
        library,
        "block-lib",
        input,
        Arc::new(NoopLibraryObserver),
    ) {
        Execution::Blocks { done, .. } => *done,
        Execution::Accepted { .. } => panic!("expected a finished block for: {input}"),
        Execution::Ignored => panic!("expected a block for: {input}"),
    }
}

fn accepted_with_library(api: &FakeApi, library: &FakeLibraryApi, input: &str) -> Block {
    match execute_with_library(
        api,
        library,
        "block-lib",
        input,
        Arc::new(NoopLibraryObserver),
    ) {
        Execution::Accepted { running } => *running,
        Execution::Blocks { done, .. } => {
            panic!("expected accepted block for {input}, got {:?}", done.status)
        }
        Execution::Ignored => panic!("expected a block for: {input}"),
    }
}

#[test]
fn library_add_reports_the_root_count() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, r"library add C:\Music");

    assert_eq!(
        library.calls(),
        vec![LibraryCall::AddRoot(r"C:\Music".to_owned())]
    );
    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("已添加扫描根"));
    assert_eq!(
        block.footer.as_deref(),
        Some("当前 1 个扫描根 · 输入 library scan 开始扫描")
    );
}

#[test]
fn library_add_rejects_a_duplicate_root() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]);
    let block = done_with_library(&api, &library, r"library add C:\Music");

    assert_eq!(block.status, BlockStatus::Error);
    let (code, message, _) = error_fields(&block);
    assert_eq!(code, "E_CMD_ARG");
    assert!(message.contains("已存在"), "{message}");
}

#[test]
fn library_scan_is_accepted_immediately() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]);
    let block = accepted_with_library(&api, &library, "library scan");

    assert_eq!(block.status, BlockStatus::Running);
    assert!(text_of_block(&block).contains("扫描已开始"));
    assert_eq!(library.calls(), vec![LibraryCall::Scan(false)]);
}

#[test]
fn library_scan_full_sets_the_flag() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]);
    let block = accepted_with_library(&api, &library, "library scan --full");

    assert!(text_of_block(&block).contains("全量扫描"));
    assert_eq!(library.calls(), vec![LibraryCall::Scan(true)]);
}

#[test]
fn library_scan_while_running_returns_busy() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]).busy();
    let block = done_with_library(&api, &library, "library scan");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_BUSY");
}

#[test]
fn library_scan_without_roots_returns_arg_error() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "library scan");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
}

#[test]
fn library_list_defaults_to_twenty_and_renders_rows() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_list_tracks(vec![
        track(1, "Kid A", "Radiohead", "/music/a.flac", 90_000),
        track(2, "Untitled", "", "/music/b.flac", 0),
    ]);
    let block = done_with_library(&api, &library, "library list");

    assert_eq!(library.calls(), vec![LibraryCall::ListRecent(20)]);
    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns, vec!["序号", "标题", "艺人", "时长"]);
    assert_eq!(rows[0], vec!["1", "Kid A", "Radiohead", "01:30"]);
    assert_eq!(rows[1][3], "--:--");
}

#[test]
fn library_list_honors_limit_and_rejects_bad_values() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, "library list --limit=5");
    assert_eq!(library.calls(), vec![LibraryCall::ListRecent(5)]);

    let block = done_with_library(&api, &library, "library list --limit=abc");
    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");

    let block = done_with_library(&api, &library, "library list --limit=0");
    assert_eq!(block.status, BlockStatus::Error);
}

#[test]
fn library_stat_renders_counts_scan_time_and_cover_cache() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_stats(Stats {
        tracks: 3,
        albums: 1,
        artists: 2,
        total_duration_ms: 3_723_000,
        last_scan_at: 0,
    });
    let block = done_with_library(&api, &library, "library stat");
    let (_, rows) = table_of_block(&block);

    assert_eq!(
        rows,
        vec![
            vec!["曲目", "3", "专辑", "1"],
            vec!["艺人", "2", "总时长", "1 小时 2 分"],
            // 3 个文件 / 1.5 MB：字节数按 1024² 换算，保留一位小数。
            vec!["最近扫描", "从未扫描", "缓存", "streams"],
        ]
    );
    assert_eq!(library.calls(), vec![LibraryCall::Stats]);
}

#[test]
fn search_renders_rows_and_remembers_results() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![
        track(1, "Kid A", "Radiohead", "/music/a.flac", 90_000),
        track(2, "Kid B", "Radiohead", "/music/b.flac", 0),
    ]);
    let block = done_with_library(&api, &library, "search kid");

    assert_eq!(
        library.calls(),
        vec![LibraryCall::Search("kid".to_owned(), 21)]
    );
    assert_eq!(library.session().len(), 2);
    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "1");
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("共 2 条"))
    );
}

#[test]
fn search_artist_only_uses_artist_as_keyword() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "晴天",
        "周杰伦",
        "/music/qingtian.flac",
        200_000,
    )]);
    let block = done_with_library(&api, &library, "search --artist 周杰伦 --local");

    assert_eq!(
        library.calls(),
        vec![LibraryCall::Search("周杰伦".to_owned(), 50)]
    );
    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2], "周杰伦");
}

#[test]
fn search_artist_filters_local_results() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![
        track(1, "A", "周杰伦", "/music/a.flac", 1_000),
        track(2, "B", "别人", "/music/b.flac", 1_000),
    ]);
    let block = done_with_library(&api, &library, "search 歌 --artist 周杰伦 --local");

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], "A");
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("共 1 条"))
    );
}

#[test]
fn search_artist_filter_empty_explains_before_and_removed() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![
        track(1, "A", "甲", "/music/a.flac", 1_000),
        track(2, "B", "乙", "/music/b.flac", 1_000),
        track(3, "C", "丙", "/music/c.flac", 1_000),
    ]);
    let block = done_with_library(&api, &library, "search 歌 --artist 周杰伦 --local");

    assert_eq!(block.status, BlockStatus::Done);
    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. }
            if text.contains("过滤前有 3 条，被 --artist 过滤掉了 3 条")
    )));
    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Search,
            len: 0,
        })
    );
}

#[test]
fn search_local_and_source_are_mutually_exclusive() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "search 晴天 --local --source netease");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(error_fields(&block).1.contains("互斥"));
    assert!(library.calls().is_empty());
}

#[test]
fn search_rejects_invalid_source() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "search 晴天 --source youtube");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(error_fields(&block).1.contains("未知音源"));
}

#[test]
fn search_rejects_out_of_range_limit() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    for input in ["search 晴天 --limit 0", "search 晴天 --limit 101"] {
        let block = done_with_library(&api, &library, input);
        assert_eq!(block.status, BlockStatus::Error, "{input}");
        assert!(error_fields(&block).1.contains("limit"), "{input}");
    }
}

#[test]
fn search_limit_applies_to_local_results() {
    let api = FakeApi::new();
    let tracks: Vec<Track> = (0..10)
        .map(|index| {
            track(
                index,
                &format!("Track {index}"),
                "Artist",
                &format!("/music/{index}.flac"),
                1_000,
            )
        })
        .collect();
    let library = FakeLibraryApi::new().with_search_tracks(tracks);
    let block = done_with_library(&api, &library, "search track --local --limit 5");

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 5);
    assert_eq!(
        library.calls(),
        vec![LibraryCall::Search("track".to_owned(), 6)]
    );
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("--limit 5"))
    );
}

#[test]
fn search_without_hits_explains_itself() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "search nothing");

    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("没有找到"));
    assert!(library.session().is_empty());
}

#[test]
fn search_truncates_results_at_default_twenty() {
    let api = FakeApi::new();
    let tracks: Vec<Track> = (0..21)
        .map(|index| {
            track(
                index,
                &format!("Track {index}"),
                "Artist",
                &format!("/music/{index}.flac"),
                1_000,
            )
        })
        .collect();
    let library = FakeLibraryApi::new().with_search_tracks(tracks);
    let block = done_with_library(&api, &library, "search track");

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 20);
    assert_eq!(library.session().len(), 20);
    assert!(text_of_block(&block).contains("只显示前 20 条"));
}

#[test]
fn local_search_sets_active_listing_with_row_count() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![
        track(1, "First", "Artist", "/music/first.flac", 1_000),
        track(2, "Second", "Artist", "/music/second.flac", 1_000),
    ]);
    done_with_library(&api, &library, "search artist --local");

    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Search,
            len: 2,
        })
    );
}

#[test]
fn empty_local_search_clears_old_context_to_zero() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    library.set_active_listing(ActiveListing {
        source: ListingSource::Search,
        len: 1,
    });

    let empty = done_with_library(&api, &library, "search nothing");
    assert_eq!(empty.status, BlockStatus::Done);
    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Search,
            len: 0,
        })
    );

    let play = done_with_library(&api, &library, "play 1");
    assert_eq!(play.status, BlockStatus::Error);
    assert!(error_fields(&play).1.contains("搜索结果只有 0 条"));
}

#[test]
fn search_requires_a_keyword() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "search");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(library.calls().is_empty());
}

#[test]
fn queue_list_replaces_search_as_active_listing() {
    let api = FakeApi::new().with_queue(queue_view(&["a.flac"], None));
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Search Song",
        "Artist",
        "/music/search.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "search song");
    done_with_library(&api, &library, "queue list");
    let block = done_with_library(&api, &library, "play 1");

    assert_eq!(
        api.calls(),
        vec![Call::QueueList, Call::QueueList, Call::QueueJump(0)]
    );
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：播放队列"))
    );
}

#[test]
fn play_uses_search_results_after_search() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![
        track(1, "First", "Artist", "/music/first.flac", 1_000),
        track(2, "Second", "Artist", "/music/second.flac", 1_000),
    ]);
    done_with_library(&api, &library, "search artist");
    let block = done_with_library(&api, &library, "play 2");

    assert_eq!(
        api.calls(),
        vec![Call::Play(Some("/music/second.flac".to_owned()))]
    );
    assert!(text_of_block(&block).contains("搜索结果第 2 首"));
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：搜索结果"))
    );
}

#[test]
fn play_number_without_any_listing_fails_with_teaching_hint() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "play 2");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(error_fields(&block).1.contains("没有可用的序号列表"));
    assert!(
        error_fields(&block)
            .2
            .is_some_and(|hint| hint.contains("search / queue list"))
    );
    assert!(api.calls().is_empty(), "不能偷偷回退到队列");
}

#[test]
fn play_search_index_out_of_range_fails() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Only",
        "Artist",
        "/music/only.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "search only");
    let block = done_with_library(&api, &library, "play 2");

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("只有 1 条"));
    assert!(api.calls().is_empty());
}

#[test]
fn library_list_sets_active_listing_for_play_number() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_list_tracks(vec![
        track(1, "First", "Artist", "/music/first.flac", 1_000),
        track(2, "Second", "Artist", "/music/second.flac", 1_000),
    ]);
    done_with_library(&api, &library, "library list");
    let block = done_with_library(&api, &library, "play 2");

    assert_eq!(
        api.calls(),
        vec![Call::Play(Some("/music/second.flac".to_owned()))]
    );
    assert!(text_of_block(&block).contains("媒体库第 2 首"));
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：媒体库"))
    );
}

#[test]
fn search_replaces_queue_as_active_listing() {
    let api = FakeApi::new().with_queue(queue_view(&["queued.flac"], None));
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Search",
        "Artist",
        "/music/search.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "queue list");
    done_with_library(&api, &library, "search song");
    let block = done_with_library(&api, &library, "play 1");

    assert_eq!(
        api.calls(),
        vec![
            Call::QueueList,
            Call::Play(Some("/music/search.flac".to_owned()))
        ]
    );
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：搜索结果"))
    );
}

#[test]
fn empty_queue_listing_gives_zero_count_error() {
    let api = FakeApi::new().with_queue(queue_view(&[], None));
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, "queue list");
    let block = done_with_library(&api, &library, "play 1");

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("播放队列只有 0 首"));
    assert!(!api.calls().contains(&Call::QueueJump(0)));
}

#[test]
fn queue_listing_out_of_range_fails_without_jump() {
    let api = FakeApi::new().with_queue(queue_view(&["only.flac"], None));
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, "queue list");
    let block = done_with_library(&api, &library, "play 2");

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("播放队列只有 1 首"));
    assert!(!api.calls().contains(&Call::QueueJump(1)));
}

fn playlist_with_items(library: &FakeLibraryApi, name: &str) -> i64 {
    library.playlist_create(name).expect("create playlist")
}

#[test]
fn playlist_create_list_and_show_set_active_playlist() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let created = done_with_library(&api, &library, r#"playlist create "周杰伦 情歌""#);
    assert_eq!(created.status, BlockStatus::Done);

    let list = done_with_library(&api, &library, "playlist list");
    assert!(
        table_of_block(&list)
            .1
            .iter()
            .any(|row| row[0] == "周杰伦 情歌")
    );

    let show = done_with_library(&api, &library, r#"playlist show "周杰伦 情歌""#);
    assert_eq!(show.status, BlockStatus::Done, "{}", text_of_block(&show));
    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Playlist("周杰伦 情歌".to_owned()),
            len: 0,
        })
    );
}

#[test]
fn playlist_add_from_search_and_show_lists_remote_metadata() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    library.remember_hits(vec![SearchHit::Online(up_track())]);
    library.set_active_listing(ActiveListing {
        source: ListingSource::Search,
        len: 1,
    });
    done_with_library(&api, &library, r#"playlist create "远程""#);
    let add = done_with_library(&api, &library, r#"playlist add "远程" 1"#);
    assert_eq!(add.status, BlockStatus::Done, "{}", text_of_block(&add));

    let show = done_with_library(&api, &library, r#"playlist show "远程""#);
    let (_, rows) = table_of_block(&show);
    assert_eq!(rows[0][1], "最长的电影");
    assert_eq!(rows[0][2], "明前奶绿");
    assert_eq!(rows[0][4], "B站");
}

#[test]
fn playlist_add_without_active_listing_teaches_user() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, r#"playlist create "A""#);
    let block = done_with_library(&api, &library, r#"playlist add "A" 1"#);
    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("当前没有可用的列表"));
    assert!(error_fields(&block).2.unwrap().contains("search"));
}

#[test]
fn playlist_add_without_index_adds_current_local_track_and_allows_duplicate() {
    let api = FakeApi::new().with_state(PlaybackState {
        track_id: Some("/music/current.flac".to_owned()),
        ..PlaybackState::default()
    });
    let library = FakeLibraryApi::new().with_track_by_path(
        "/music/current.flac",
        track(1, "正在播放", "艺人", "/music/current.flac", 1_000),
    );
    done_with_library(&api, &library, r#"playlist create "A""#);

    let first = done_with_library(&api, &library, r#"playlist add "A""#);
    assert_eq!(first.status, BlockStatus::Done, "{}", text_of_block(&first));
    let second = done_with_library(&api, &library, r#"playlist add "A""#);
    assert_eq!(second.status, BlockStatus::Done);
    assert!(text_of_block(&second).contains("已在歌单中"));

    let show = done_with_library(&api, &library, r#"playlist show "A""#);
    let (_, rows) = table_of_block(&show);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][1], "! 正在播放");
}

#[test]
fn playlist_add_without_current_or_listing_teaches_user() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, r#"playlist create "A""#);
    let block = done_with_library(&api, &library, r#"playlist add "A""#);

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("当前没有正在播放的曲目"));
    assert!(
        error_fields(&block)
            .2
            .is_some_and(|hint| hint.contains("search / queue list / playlist show"))
    );
}

#[test]
fn playlist_add_without_index_adds_current_remote_track() {
    let api = FakeApi::new().with_state(PlaybackState {
        track_id: Some("/cache/remote.m4a".to_owned()),
        ..PlaybackState::default()
    });
    let mut remote = track(0, "远程", "UP", "/cache/remote.m4a", 2_000);
    remote.kind = "remote".to_owned();
    remote.source = "bilibili".to_owned();
    remote.source_id = "BVremote".to_owned();
    let library = FakeLibraryApi::new().with_track_by_path("/cache/remote.m4a", remote);
    done_with_library(&api, &library, r#"playlist create "A""#);
    done_with_library(&api, &library, r#"playlist add "A""#);

    let show = done_with_library(&api, &library, r#"playlist show "A""#);
    let (_, rows) = table_of_block(&show);
    assert_eq!(rows[0][1], "远程");
    assert_eq!(rows[0][4], "B站");
}

#[test]
fn playlist_add_from_queue_listing_with_index() {
    let api = FakeApi::new().with_queue(queue_view(&["/music/q.flac"], None));
    let library = FakeLibraryApi::new().with_track_by_path(
        "/music/q.flac",
        track(1, "队列歌", "艺人", "/music/q.flac", 1_000),
    );
    done_with_library(&api, &library, "queue list");
    done_with_library(&api, &library, r#"playlist create "A""#);
    let add = done_with_library(&api, &library, r#"playlist add "A" 1"#);
    assert_eq!(add.status, BlockStatus::Done, "{}", text_of_block(&add));
    let show = done_with_library(&api, &library, r#"playlist show "A""#);
    assert_eq!(table_of_block(&show).1[0][1], "! 队列歌");
}

#[test]
fn playlist_add_from_playlist_listing_with_index() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let source = playlist_with_items(&library, "源");
    library
        .playlist_add(
            source,
            &NewPlaylistItem {
                kind: "remote".to_owned(),
                path: String::new(),
                source: "netease".to_owned(),
                source_id: "10001".to_owned(),
                title: "网易云歌".to_owned(),
                artist: "艺人".to_owned(),
                album: String::new(),
                duration_ms: 1_000,
            },
        )
        .expect("item");
    done_with_library(&api, &library, r#"playlist show "源""#);
    done_with_library(&api, &library, r#"playlist create "目标""#);
    let add = done_with_library(&api, &library, r#"playlist add "目标" 1"#);
    assert_eq!(add.status, BlockStatus::Done, "{}", text_of_block(&add));
    let show = done_with_library(&api, &library, r#"playlist show "目标""#);
    assert_eq!(table_of_block(&show).1[0][4], "网易云");
}

#[test]
fn playlist_rename_and_delete_commands() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, r#"playlist create "旧""#);
    let rename = done_with_library(&api, &library, r#"playlist rename "旧" "新""#);
    assert_eq!(
        rename.status,
        BlockStatus::Done,
        "{}",
        text_of_block(&rename)
    );
    let show = done_with_library(&api, &library, r#"playlist show "新""#);
    assert_eq!(show.status, BlockStatus::Done);
    let delete = done_with_library(&api, &library, r#"playlist delete "新""#);
    assert_eq!(delete.status, BlockStatus::Done);
    assert!(library.playlist_list().expect("list").is_empty());
}

#[test]
fn playlist_add_from_library_listing() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_list_tracks(vec![track(
        1,
        "库里的歌",
        "艺人",
        "/music/lib.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "library list");
    done_with_library(&api, &library, r#"playlist create "A""#);
    let add = done_with_library(&api, &library, r#"playlist add "A" 1"#);
    assert_eq!(add.status, BlockStatus::Done, "{}", text_of_block(&add));
    let show = done_with_library(&api, &library, r#"playlist show "A""#);
    assert_eq!(table_of_block(&show).1[0][1], "! 库里的歌");
}

#[test]
fn playlist_add_out_of_range_is_human_error() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "一",
        "艺人",
        "/music/one.flac",
        1_000,
    )]);
    done_with_library(&api, &library, "search 歌");
    done_with_library(&api, &library, r#"playlist create "A""#);
    let add = done_with_library(&api, &library, r#"playlist add "A" 2"#);
    assert_eq!(add.status, BlockStatus::Error);
    assert!(error_fields(&add).1.contains("搜索结果只有 1 条"));
}

#[test]
fn playlist_play_defaults_to_first_item() {
    let dir = TestDir::new("playlist-default-play");
    let path = dir.path().join("first.flac");
    fs::write(&path, b"fake").expect("audio");
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let id = playlist_with_items(&library, "A");
    library
        .playlist_add(
            id,
            &NewPlaylistItem {
                kind: "local".to_owned(),
                path: path.to_string_lossy().into_owned(),
                source: String::new(),
                source_id: String::new(),
                title: "第一".to_owned(),
                artist: String::new(),
                album: String::new(),
                duration_ms: 1_000,
            },
        )
        .expect("add");
    let block = done_with_library(&api, &library, r#"playlist play "A""#);
    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("第 1 首"));
}

#[test]
fn playlist_remove_keeps_one_based_positions_contiguous() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let id = playlist_with_items(&library, "A");
    for title in ["一", "二", "三"] {
        library
            .playlist_add(
                id,
                &NewPlaylistItem {
                    kind: "local".to_owned(),
                    path: format!("/music/{title}.flac"),
                    source: String::new(),
                    source_id: String::new(),
                    title: title.to_owned(),
                    artist: String::new(),
                    album: String::new(),
                    duration_ms: 1_000,
                },
            )
            .expect("add");
    }
    let removed = done_with_library(&api, &library, r#"playlist remove "A" 1"#);
    assert_eq!(removed.status, BlockStatus::Done);
    let show = done_with_library(&api, &library, r#"playlist show "A""#);
    let (_, rows) = table_of_block(&show);
    assert_eq!(rows[0][0], "1");
    assert_eq!(rows[0][1], "! 二");
    assert_eq!(rows[1][0], "2");
    assert_eq!(rows[1][1], "! 三");
}

#[test]
fn playlist_play_local_missing_file_is_human_error() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let id = playlist_with_items(&library, "A");
    library
        .playlist_add(
            id,
            &NewPlaylistItem {
                kind: "local".to_owned(),
                path: "/does/not/exist.flac".to_owned(),
                source: String::new(),
                source_id: String::new(),
                title: "不存在".to_owned(),
                artist: String::new(),
                album: String::new(),
                duration_ms: 1_000,
            },
        )
        .expect("add");
    let block = done_with_library(&api, &library, r#"playlist play "A""#);
    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("文件不存在"));
}

#[test]
fn playlist_play_local_and_play_number_after_show() {
    let dir = TestDir::new("playlist-local-play");
    let path = dir.path().join("song.flac");
    fs::write(&path, b"fake").expect("audio");
    let path_text = path.to_string_lossy().into_owned();
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let id = playlist_with_items(&library, "A");
    library
        .playlist_add(
            id,
            &NewPlaylistItem {
                kind: "local".to_owned(),
                path: path_text.clone(),
                source: String::new(),
                source_id: String::new(),
                title: "本地".to_owned(),
                artist: "艺人".to_owned(),
                album: String::new(),
                duration_ms: 1_000,
            },
        )
        .expect("add");
    let direct = done_with_library(&api, &library, r#"playlist play "A" 1"#);
    assert_eq!(direct.status, BlockStatus::Done);
    assert!(text_of_block(&direct).contains("第 1 首"));
    done_with_library(&api, &library, r#"playlist show "A""#);
    let by_number = done_with_library(&api, &library, "play 1");
    assert_eq!(by_number.status, BlockStatus::Done);
    assert!(
        api.calls()
            .iter()
            .filter(|call| matches!(call, Call::Play(Some(path)) if path == &path_text))
            .count()
            >= 2
    );
}

#[test]
fn playlist_play_remote_uses_prepare() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    library.remember_hits(vec![SearchHit::Online(up_track())]);
    library.set_active_listing(ActiveListing {
        source: ListingSource::Search,
        len: 1,
    });
    done_with_library(&api, &library, r#"playlist create "远程""#);
    done_with_library(&api, &library, r#"playlist add "远程" 1"#);
    let cached = PathBuf::from("/cache/remote.m4a");
    let online = FakeQueueOnline {
        path: cached.clone(),
    };
    let block = done_with_playlist_online(&api, &library, &online, r#"playlist play "远程""#);
    assert_eq!(block.status, BlockStatus::Done, "{}", text_of_block(&block));
    assert!(
        api.calls().iter().any(
            |call| matches!(call, Call::Play(Some(path)) if path == &cached.to_string_lossy())
        )
    );
    assert!(text_of_block(&block).contains("来源：B站"));
}

#[test]
fn playlist_play_out_of_range_is_human_error() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    playlist_with_items(&library, "A");
    let block = done_with_library(&api, &library, r#"playlist play "A" 1"#);
    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("只有 0 首"));
}

#[test]
fn playlist_unknown_name_and_duplicate_name_errors() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    done_with_library(&api, &library, r#"playlist create "A""#);
    let duplicate = done_with_library(&api, &library, r#"playlist create "a""#);
    assert_eq!(duplicate.status, BlockStatus::Error);
    assert!(error_fields(&duplicate).1.contains("已存在"));
    let missing = done_with_library(&api, &library, r#"playlist show "没有""#);
    assert_eq!(missing.status, BlockStatus::Error);
    assert!(error_fields(&missing).1.contains("不存在"));
}

#[test]
fn library_alias_runs_the_same_subcommand() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "lib stat");

    assert_eq!(block.status, BlockStatus::Done);
    assert_eq!(library.calls(), vec![LibraryCall::Stats]);
}

// ---- 扫描进度（节流 + 块状态） ----

fn event_observer(now: Arc<AtomicU64>, emitted: Arc<Mutex<Vec<Block>>>) -> EventObserver {
    let emit: Arc<EmitFn> = Arc::new(move |block: &Block| {
        emitted.lock().expect("emitted mutex").push(block.clone());
    });
    let clock: Arc<ClockFn> = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    EventObserver::with_port(
        emit,
        clock,
        "scan-block".to_owned(),
        "library scan".to_owned(),
        1_000,
    )
}

fn progress_current(block: &Block) -> u64 {
    block
        .outputs
        .iter()
        .find_map(|output| match output {
            Output::Progress { current, .. } => Some(*current),
            _ => None,
        })
        .expect("progress output")
}

#[test]
fn progress_is_throttled_by_time_not_by_file_count() {
    let now = Arc::new(AtomicU64::new(1_000));
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

    for (at, scanned) in [
        (1_000, 1),
        (1_100, 2),
        (1_400, 3),
        (1_500, 4),
        (1_900, 5),
        (2_000, 6),
    ] {
        now.store(at, Ordering::SeqCst);
        observer.on_progress(scanned, 10, Path::new("song.flac"));
    }

    let blocks = emitted.lock().expect("emitted mutex");
    let currents: Vec<u64> = blocks.iter().map(progress_current).collect();
    // 1 Hz 下限：只在 0ms、500ms、1000ms 三个时间点放行。
    assert_eq!(currents, vec![1, 4, 6]);
    assert!(blocks.iter().all(|block| block.id == "scan-block"));
    assert!(
        blocks
            .iter()
            .all(|block| block.status == BlockStatus::Running)
    );
}

#[test]
fn scan_finished_block_summarizes_the_report() {
    let now = Arc::new(AtomicU64::new(1_000));
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

    now.store(1_750, Ordering::SeqCst);
    observer.on_finished(&ScanReport {
        scanned: 10,
        indexed: 8,
        skipped: 1,
        failed: 1,
        missing: 2,
        cancelled: false,
    });

    let blocks = emitted.lock().expect("emitted mutex");
    let block = blocks.last().expect("finished block");
    assert_eq!(block.id, "scan-block");
    assert_eq!(block.status, BlockStatus::Done);
    assert_eq!(block.duration_ms, Some(750));
    let text = block
        .outputs
        .iter()
        .filter_map(|output| match output {
            Output::Text { spans, .. } => Some(
                spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("扫描完成"), "{text}");
    assert!(text.contains("扫描 10"), "{text}");
    assert!(text.contains("入库 8"), "{text}");
    assert!(text.contains("缺失 2"), "{text}");
}

#[test]
fn cancelled_scan_finishes_as_cancelled() {
    let now = Arc::new(AtomicU64::new(1_000));
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

    observer.on_finished(&ScanReport {
        scanned: 3,
        indexed: 1,
        cancelled: true,
        ..ScanReport::default()
    });

    let blocks = emitted.lock().expect("emitted mutex");
    let block = blocks.last().expect("cancelled block");
    assert_eq!(block.status, BlockStatus::Cancelled);
}

#[test]
fn scan_failure_block_carries_the_error_code() {
    let now = Arc::new(AtomicU64::new(1_000));
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

    observer.on_failed(&CmdError::new("E_LIB_IO", "无法读取目录"));

    let blocks = emitted.lock().expect("emitted mutex");
    let block = blocks.last().expect("failure block");
    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(block).0, "E_LIB_IO");
}

#[test]
fn cmd_error_payload_serializes_without_a_missing_hint() {
    let reply = CmdErrorPayload::from(CmdError::new("E_BUSY", "已有扫描在进行中"));
    assert_eq!(
        serde_json::to_string(&reply).unwrap(),
        r#"{"code":"E_BUSY","message":"已有扫描在进行中"}"#
    );
}

#[test]
fn reply_body_is_camel_case_json() {
    let reply = CmdExecReply {
        block_id: "block-1".to_owned(),
        ok: true,
    };
    assert_eq!(
        serde_json::to_string(&reply).unwrap(),
        r#"{"blockId":"block-1","ok":true}"#
    );
}

#[test]
fn scan_started_block_is_running_with_the_expected_id() {
    let block = scan_started_block("blk-scan", "library scan --full", true);

    assert_eq!(block.id, "blk-scan");
    assert_eq!(block.input, "library scan --full");
    assert_eq!(block.status, BlockStatus::Running);
    assert!(text_of_block(&block).contains("扫描已开始"));
    assert!(text_of_block(&block).contains("全量扫描"));
}

fn table_of_block(block: &Block) -> (Vec<&str>, Vec<Vec<&str>>) {
    block
        .outputs
        .iter()
        .find_map(|output| match output {
            Output::Table { columns, rows, .. } => Some((
                columns.iter().map(String::as_str).collect(),
                rows.iter()
                    .map(|row| row.iter().map(String::as_str).collect())
                    .collect(),
            )),
            _ => None,
        })
        .expect("table output")
}

// ---- IPC 与事件 ----

#[test]
fn terminal_event_channel_uses_a_colon_not_a_dot() {
    assert_eq!(EVENT_TERMINAL_BLOCK, "terminal:block");
    assert!(!EVENT_TERMINAL_BLOCK.contains('.'));
}

// ---- 补全（T-0020） ----

#[test]
fn complete_maps_dynamic_queue_indices_and_polls_snapshot_once() {
    let api = FakeApi::new().with_state(PlaybackState {
        queue_length: Some(3),
        ..PlaybackState::default()
    });

    let completion = complete_for(&api, "play ", 5);

    assert_eq!(api.calls(), vec![Call::Snapshot]);
    assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
    let texts: Vec<&str> = completion
        .items
        .iter()
        .map(|item| item.text.as_str())
        .collect();
    assert_eq!(texts, ["1", "2", "3"]);
}

#[test]
fn complete_degrades_to_empty_dynamic_candidates_when_snapshot_fails() {
    let api = FakeApi::new().with_playback_error(CmdError::new("E_BUSY", "播放器线程不可用"));

    let completion = complete_for(&api, "play ", 5);

    // 降级：不报错、不阻塞，只返回空列表；区间仍然给出。
    assert!(completion.items.is_empty());
    assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
}

#[test]
fn complete_with_unknown_queue_length_returns_no_dynamic_candidates() {
    // FakeApi 默认 `queue_length: None`：快照成功但长度未知，同样降级为空。
    let api = FakeApi::new();

    let completion = complete_for(&api, "play ", 5);

    assert!(completion.items.is_empty());
}

#[test]
fn complete_returns_static_candidates_without_polling_the_player() {
    let api = FakeApi::new();

    let completion = complete_for(&api, "pl", 2);

    assert_eq!(api.calls(), Vec::new());
    let play = completion
        .items
        .iter()
        .find(|item| item.text == "play")
        .expect("play candidate");
    assert_eq!(play.kind, np_core::command::CompletionKind::Command);
}

// ---- 在线音源：注册表 / 缓存 / 去重 / 错误映射（T-0038） ----

struct TestDir {
    path: PathBuf,
}

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

impl TestDir {
    fn new(label: &str) -> Self {
        let unique = format!(
            "np-command-{label}-{}-{}",
            std::process::id(),
            TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy)]
enum ResolvePlan {
    Expired403,
    Expired410,
    Network,
    Copyright,
}

impl ResolvePlan {
    fn into_error(self) -> SourceError {
        match self {
            Self::Expired403 => SourceError::Http(403),
            Self::Expired410 => SourceError::Http(410),
            Self::Network => SourceError::Network("连接失败".to_string()),
            Self::Copyright => SourceError::Unavailable(
                "该曲目在网易云不可播放（可能版权受限或需要会员）".to_string(),
            ),
        }
    }
}

struct FakeOnlineSource {
    id: SourceId,
    resolves: AtomicUsize,
    searches: AtomicUsize,
    plans: Mutex<Vec<ResolvePlan>>,
    stream: ResolvedStream,
    delay: Duration,
}

impl FakeOnlineSource {
    fn new(id: SourceId, plans: Vec<ResolvePlan>) -> Arc<Self> {
        Arc::new(Self {
            id,
            resolves: AtomicUsize::new(0),
            searches: AtomicUsize::new(0),
            plans: Mutex::new(plans),
            stream: ResolvedStream {
                url: "https://example.invalid/audio".to_string(),
                mime: "audio/mpeg".to_string(),
                expires_at: None,
                headers: Vec::new(),
            },
            delay: Duration::ZERO,
        })
    }

    fn with_delay(self: &Arc<Self>, delay: Duration) -> Arc<Self> {
        let mut cloned = FakeOnlineSource {
            id: self.id,
            resolves: AtomicUsize::new(0),
            searches: AtomicUsize::new(0),
            plans: Mutex::new(Vec::new()),
            stream: self.stream.clone(),
            delay,
        };
        cloned.plans = Mutex::new(
            self.plans
                .lock()
                .expect("plans mutex")
                .iter()
                .copied()
                .collect(),
        );
        Arc::new(cloned)
    }

    fn resolves(&self) -> usize {
        self.resolves.load(Ordering::SeqCst)
    }

    fn searches(&self) -> usize {
        self.searches.load(Ordering::SeqCst)
    }
}

impl AudioSource for FakeOnlineSource {
    fn id(&self) -> SourceId {
        self.id
    }

    fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SourceTrack>, SourceError> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn resolve(&self, _track: &SourceTrack) -> Result<ResolvedStream, SourceError> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let mut plans = self.plans.lock().expect("plans mutex");
        if plans.is_empty() {
            return Ok(self.stream.clone());
        }
        Err(plans.remove(0).into_error())
    }
}

struct FakeDownloader {
    calls: AtomicUsize,
    bytes: Vec<u8>,
    fail: bool,
    progress: Vec<u64>,
}

impl FakeDownloader {
    fn new(bytes: &[u8], fail: bool, progress: Vec<u64>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            bytes: bytes.to_vec(),
            fail,
            progress,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Downloader for FakeDownloader {
    fn download(
        &self,
        stream: &ResolvedStream,
        dest: &Path,
    ) -> Result<DownloadOutcome, SourceError> {
        self.download_with_progress(stream, dest, &mut |_| {})
    }

    fn download_with_progress(
        &self,
        _stream: &ResolvedStream,
        dest: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<DownloadOutcome, SourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(SourceError::Download("模拟中断".to_string()));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| SourceError::Download(error.to_string()))?;
        }
        fs::write(dest, &self.bytes).map_err(|error| SourceError::Download(error.to_string()))?;
        for written in &self.progress {
            progress(*written);
        }
        Ok(DownloadOutcome::Downloaded {
            bytes: self.bytes.len() as u64,
        })
    }
}

fn online_track(source: SourceId, id: &str, title: &str) -> SourceTrack {
    SourceTrack {
        source,
        source_id: id.to_string(),
        title: title.to_string(),
        artist: "在线艺人".to_string(),
        duration_ms: 200_000,
        album: None,
        cover_url: None,
    }
}

fn online_service(
    source: Arc<FakeOnlineSource>,
    downloader: Arc<FakeDownloader>,
) -> (TestDir, OnlineService) {
    let dir = TestDir::new("online");
    let cache = StreamCache::with_limits(
        dir.path(),
        Arc::clone(&downloader) as Arc<dyn Downloader>,
        10 * 1024 * 1024,
        5 * 1024 * 1024,
    );
    let registry = SourceRegistry::new(vec![source as Arc<dyn AudioSource>]);
    (dir, OnlineService::from_parts(registry, cache))
}

#[test]
fn run_online_search_default_caps_local_at_ten() {
    let local_tracks: Vec<Track> = (0..20)
        .map(|index| {
            track(
                index,
                &format!("Local {index}"),
                "Artist",
                &format!("/music/{index}.flac"),
                1_000,
            )
        })
        .collect();
    let library = FakeLibraryApi::new().with_search_tracks(local_tracks);
    let online_tracks: Vec<SourceTrack> = (0..20)
        .map(|index| online_track(SourceId::Bilibili, &format!("BV{index}"), "Bili"))
        .collect();
    let online = FakeOnline {
        tracks: online_tracks,
        all_failed: false,
    };
    let options = SearchOptions {
        keyword: "x".to_owned(),
        artist: None,
        source: None,
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: true,
        up: None,
        mid: None,
        no_filter: false,
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_search_with_options(
        &options,
        &online,
        &library,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    let (_, rows) = table_of_block(done);
    assert_eq!(rows.len(), 20);
    assert_eq!(rows[0][1], "本地");
    assert_eq!(rows[9][1], "本地");
    assert_eq!(rows[10][1], "B站");
    assert_eq!(rows[19][1], "B站");
}

#[test]
fn run_online_search_source_excludes_local_and_other_sources() {
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Local",
        "Artist",
        "/music/local.flac",
        1_000,
    )]);
    let online = FakeOnline {
        tracks: vec![
            online_track(SourceId::Bilibili, "BV1xx", "Bili"),
            online_track(SourceId::Netease, "10001", "Net"),
        ],
        all_failed: false,
    };
    let options = SearchOptions {
        keyword: "x".to_owned(),
        artist: None,
        source: Some(SourceId::Bilibili),
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: true,
        up: None,
        mid: None,
        no_filter: false,
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_search_with_options(
        &options,
        &online,
        &library,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    let (_, rows) = table_of_block(done);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], "B站");
    assert!(library.calls().is_empty(), "source 搜索不应查本地");
}

#[test]
fn run_online_search_artist_empty_explains_filter() {
    let library = FakeLibraryApi::new();
    let online = FakeOnline {
        tracks: vec![online_track(SourceId::Bilibili, "BV1xx", "Bili")],
        all_failed: false,
    };
    let options = SearchOptions {
        keyword: "歌".to_owned(),
        artist: Some("周杰伦".to_owned()),
        source: None,
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: true,
        up: None,
        mid: None,
        no_filter: false,
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_search_with_options(
        &options,
        &online,
        &library,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    assert!(done.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. }
            if text.contains("过滤前有 3 条，被 --artist 过滤掉了 3 条")
    )));
    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Search,
            len: 0,
        })
    );
}

fn resolved_up(name: &str, mid: u64, same_name: usize) -> UpResolution {
    UpResolution {
        mid,
        name: Some(name.to_owned()),
        fans: 100,
        same_name,
    }
}

#[test]
fn recall_a_enough_stops_before_b_and_d() {
    let library = FakeLibraryApi::new();
    let tracks: Vec<SourceTrack> = (0..20)
        .map(|index| {
            let mut track = online_track(SourceId::Bilibili, &format!("BV{index}"), "晴天");
            track.artist = "周杰伦".to_owned();
            track
        })
        .collect();
    let online = ChannelOnline::new(
        HashMap::from([("晴天".to_owned(), tracks)]),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应解析")),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应取空间")),
    );
    let options = artist_options("晴天", "周杰伦", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(table_of_block(&block).1.len(), 20);
    assert_eq!(online.calls(), vec![("晴天".to_owned(), None)]);
    assert_eq!(online.resolve_calls.load(Ordering::SeqCst), 0);
    assert_eq!(online.archive_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn recall_d_before_c_and_space_hit_without_artist_name_in_title() {
    let library = FakeLibraryApi::new();
    let online = ChannelOnline::new(
        HashMap::from([
            ("花骨朵".to_owned(), Vec::new()),
            ("缪尔Miru 花骨朵".to_owned(), Vec::new()),
        ]),
        Ok(resolved_up("缪尔Miru", 42, 1)),
        Ok(vec![{
            let mut track = online_track(SourceId::Bilibili, "BVlow", "花骨朵");
            track.artist = "缪尔Miru".to_owned();
            track
        }]),
    );
    let options = artist_options("花骨朵", "缪尔Miru", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2], "花骨朵");
    assert_eq!(rows[0][3], "缪尔Miru");
    assert_eq!(
        online.calls(),
        vec![
            ("花骨朵".to_owned(), None),
            ("缪尔Miru 花骨朵".to_owned(), Some(SourceId::Bilibili)),
        ]
    );
    assert_eq!(online.resolve_calls.load(Ordering::SeqCst), 1);
    assert_eq!(online.archive_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn recall_d_enough_skips_c_after_b() {
    let library = FakeLibraryApi::new();
    let mut d_track = online_track(SourceId::Bilibili, "BVd", "花骨朵");
    d_track.artist = "缪尔Miru".to_owned();
    let online = ChannelOnline::new(
        HashMap::new(),
        Ok(resolved_up("缪尔Miru", 42, 1)),
        Ok(vec![d_track]),
    );
    let options = artist_options("花骨朵", "缪尔Miru", 1);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(table_of_block(&block).1.len(), 1);
    assert_eq!(
        online.calls(),
        vec![
            ("花骨朵".to_owned(), None),
            ("缪尔Miru 花骨朵".to_owned(), Some(SourceId::Bilibili)),
        ]
    );
    assert_eq!(online.archive_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn recall_c_is_fallback_when_d_cannot_resolve_and_reports_it() {
    let library = FakeLibraryApi::new();
    let mut c_track = online_track(SourceId::Bilibili, "BVc", "花骨朵");
    c_track.artist = "缪尔Miru".to_owned();
    let online = ChannelOnline::new(
        HashMap::from([("缪尔Miru".to_owned(), vec![c_track])]),
        Ok(resolved_up("缪尔Miru", 0, 0)),
        Ok(Vec::new()),
    );
    let options = artist_options("花骨朵", "缪尔Miru", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(
        online.calls(),
        vec![
            ("花骨朵".to_owned(), None),
            ("缪尔Miru 花骨朵".to_owned(), Some(SourceId::Bilibili)),
            ("缪尔Miru".to_owned(), Some(SourceId::Bilibili)),
        ]
    );
    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. }
            if text == "未能解析到该艺人对应的 UP，已退回普通搜索"
    )));
    assert_eq!(table_of_block(&block).1.len(), 1);
}

#[test]
fn recall_d_412_reports_and_skips_c() {
    let library = FakeLibraryApi::new();
    let online = ChannelOnline::new(
        HashMap::new(),
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "B 站限流，请稍后再试（未自动重试）",
        )),
        Ok(vec![online_track(SourceId::Bilibili, "BVx", "花骨朵")]),
    );
    let options = artist_options("花骨朵", "缪尔Miru", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(
        online.calls(),
        vec![
            ("花骨朵".to_owned(), None),
            ("缪尔Miru 花骨朵".to_owned(), Some(SourceId::Bilibili)),
        ]
    );
    assert_eq!(online.resolve_calls.load(Ordering::SeqCst), 1);
    assert_eq!(online.archive_calls.load(Ordering::SeqCst), 0);
    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. } if text.contains("空间检索被限流，本次只用普通搜索")
    )));
}

#[test]
fn recall_d_archives_412_reports_and_skips_c() {
    let library = FakeLibraryApi::new();
    let online = ChannelOnline::new(
        HashMap::new(),
        Ok(resolved_up("缪尔Miru", 42, 1)),
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "B 站限流，请稍后再试（未自动重试）",
        )),
    );
    let options = artist_options("花骨朵", "缪尔Miru", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(online.archive_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        online.calls(),
        vec![
            ("花骨朵".to_owned(), None),
            ("缪尔Miru 花骨朵".to_owned(), Some(SourceId::Bilibili)),
        ]
    );
    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. } if text.contains("空间检索被限流，本次只用普通搜索")
    )));
}

#[test]
fn recall_stops_b_and_c_after_4s_but_still_tries_d() {
    let library = FakeLibraryApi::new();
    let online = ChannelOnline::new(
        HashMap::new(),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应解析")),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应取空间")),
    );
    let options = artist_options("花骨朵", "缪尔Miru", DEFAULT_SEARCH_RESULTS);
    let calls = Arc::new(AtomicUsize::new(0));
    let clock_calls = Arc::clone(&calls);
    let clock = move || {
        if clock_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            0
        } else {
            5_000
        }
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    run_online_search_with_options(
        &options,
        &online,
        &library,
        &emit,
        &running_template(),
        &clock,
    );

    assert_eq!(online.calls(), vec![("花骨朵".to_owned(), None)]);
    assert_eq!(online.resolve_calls.load(Ordering::SeqCst), 1);
    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    assert!(done.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. } if text.contains("超过 4s")
    )));
}

#[test]
fn recall_dedupes_same_source_id_across_channels() {
    let library = FakeLibraryApi::new();
    let mut track = online_track(SourceId::Bilibili, "BVsame", "晴天");
    track.artist = "周杰伦".to_owned();
    let online = ChannelOnline::new(
        HashMap::from([
            ("晴天".to_owned(), vec![track.clone()]),
            ("周杰伦 晴天".to_owned(), vec![track.clone()]),
            ("周杰伦".to_owned(), vec![track]),
        ]),
        Ok(resolved_up("周杰伦", 0, 0)),
        Ok(Vec::new()),
    );
    let options = artist_options("晴天", "周杰伦", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(table_of_block(&block).1.len(), 1);
}

#[test]
fn multi_keyword_filter_requires_each_word_not_contiguous_phrase() {
    let library = FakeLibraryApi::new();
    let mut track = online_track(SourceId::Bilibili, "BVmulti", "第一次 唱 文静");
    track.artist = "明前奶绿".to_owned();
    let online = ChannelOnline::new(
        HashMap::from([("文静 第一次".to_owned(), vec![track])]),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应解析")),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应取空间")),
    );
    let options = artist_options("文静 第一次", "明前奶绿", 1);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(table_of_block(&block).1.len(), 1);
    assert_eq!(online.calls(), vec![("文静 第一次".to_owned(), None)]);
}

#[test]
fn no_filter_keeps_source_raw_results() {
    let library = FakeLibraryApi::new();
    let mut raw = online_track(SourceId::Bilibili, "BVraw", "完全无关");
    raw.artist = "别人".to_owned();
    let online = ChannelOnline::new(
        HashMap::from([("晴天".to_owned(), vec![raw])]),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应解析")),
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应取空间")),
    );
    let mut options = artist_options("晴天", "周杰伦", 1);
    options.no_filter = true;

    let block = run_online_capture(&options, &online, &library);

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2], "完全无关");
    assert_eq!(online.calls(), vec![("晴天".to_owned(), None)]);
}

#[test]
fn run_online_search_default_footer_names_per_source_ten() {
    let library = FakeLibraryApi::new();
    let online = FakeOnline {
        tracks: vec![online_track(SourceId::Netease, "10001", "Net")],
        all_failed: false,
    };
    let options = SearchOptions {
        keyword: "歌".to_owned(),
        artist: None,
        source: None,
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: true,
        up: None,
        mid: None,
        no_filter: false,
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_search_with_options(
        &options,
        &online,
        &library,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    assert!(
        done.footer
            .as_deref()
            .is_some_and(|footer| footer.contains("每源 10，--limit 可调"))
    );
}

fn run_online_capture(
    options: &SearchOptions,
    online: &dyn OnlineApi,
    library: &FakeLibraryApi,
) -> Block {
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    run_online_search_with_options(
        options,
        online,
        library,
        &emit,
        &running_template(),
        &|| 2_000,
    );
    emitted
        .lock()
        .expect("emitted")
        .last()
        .expect("done block")
        .clone()
}

fn artist_options(keyword: &str, artist: &str, limit: usize) -> SearchOptions {
    SearchOptions {
        keyword: keyword.to_owned(),
        artist: Some(artist.to_owned()),
        source: None,
        limit,
        local: false,
        explicit_limit: false,
        has_keyword: true,
        up: None,
        mid: None,
        no_filter: false,
    }
}

#[test]
fn artist_query_is_artist_first_and_pool_expands() {
    let library = FakeLibraryApi::new();
    let mut author = online_track(SourceId::Bilibili, "BV1xx", "晴天");
    author.artist = "周杰伦".to_owned();
    let online = RecordingOnline::new(vec![author]);
    let options = artist_options("晴天", "周杰伦", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(
        online.calls(),
        vec![
            ("晴天".to_owned(), 50, None),
            ("周杰伦 晴天".to_owned(), 50, Some(SourceId::Bilibili)),
            ("周杰伦".to_owned(), 50, Some(SourceId::Bilibili)),
        ]
    );
    assert_eq!(table_of_block(&block).1.len(), 1);
}

#[test]
fn explicit_limit_scales_artist_pool_by_three() {
    let library = FakeLibraryApi::new();
    let online = RecordingOnline::new(Vec::new());
    let options = artist_options("晴天", "周杰伦", 5);

    run_online_capture(&options, &online, &library);

    assert_eq!(online.calls()[0].1, 15);
}

#[test]
fn author_match_sorts_before_title_match() {
    let library = FakeLibraryApi::new();
    let mut title_match = online_track(SourceId::Bilibili, "BVtitle", "上传者A");
    title_match.title = "【周杰伦】晴天 翻唱".to_owned();
    title_match.artist = "上传者A".to_owned();
    let mut author_match = online_track(SourceId::Bilibili, "BVauthor", "晴天");
    author_match.artist = "周杰伦".to_owned();
    let online = RecordingOnline::new(vec![title_match, author_match]);
    let options = artist_options("晴天", "周杰伦", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][3], "周杰伦");
    assert_eq!(rows[1][3], "上传者A");
}

#[test]
fn bilibili_title_match_is_kept() {
    let library = FakeLibraryApi::new();
    let mut track = online_track(SourceId::Bilibili, "BVtitle", "切片号");
    track.title = "【明前奶绿】不为谁而作的歌".to_owned();
    track.artist = "切片号".to_owned();
    let online = RecordingOnline::new(vec![track]);
    let options = artist_options("不为谁而作的歌", "明前奶绿", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][3], "切片号");
}

#[test]
fn bilibili_title_must_also_contain_keyword_when_both_given() {
    let library = FakeLibraryApi::new();
    let mut track = online_track(SourceId::Bilibili, "BVother", "切片号");
    track.title = "【明前奶绿】别的歌".to_owned();
    track.artist = "切片号".to_owned();
    let online = RecordingOnline::new(vec![track]);
    let options = artist_options("不为谁而作的歌", "明前奶绿", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. }
            if text.contains("过滤前有 3 条，被 --artist 过滤掉了 3 条")
    )));
}

#[test]
fn artist_only_keeps_title_without_keyword_constraint() {
    let library = FakeLibraryApi::new();
    let mut track = online_track(SourceId::Bilibili, "BVother", "切片号");
    track.title = "【明前奶绿】别的歌".to_owned();
    track.artist = "切片号".to_owned();
    let online = RecordingOnline::new(vec![track]);
    let options = SearchOptions {
        keyword: "明前奶绿".to_owned(),
        artist: Some("明前奶绿".to_owned()),
        source: None,
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: false,
        up: None,
        mid: None,
        no_filter: false,
    };

    let block = run_online_capture(&options, &online, &library);

    assert_eq!(table_of_block(&block).1.len(), 1);
}

#[test]
fn netease_title_match_is_not_kept() {
    let library = FakeLibraryApi::new();
    let mut track = online_track(SourceId::Netease, "10001", "别的艺人");
    track.title = "明前奶绿 翻唱".to_owned();
    let online = RecordingOnline::new(vec![track]);
    let options = artist_options("歌", "明前奶绿", DEFAULT_SEARCH_RESULTS);

    let block = run_online_capture(&options, &online, &library);

    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. }
            if text.contains("过滤前有 1 条，被 --artist 过滤掉了 1 条")
    )));
}

#[test]
fn no_artist_keeps_per_source_pool() {
    let library = FakeLibraryApi::new();
    let online = RecordingOnline::new(Vec::new());
    let options = SearchOptions {
        keyword: "晴天".to_owned(),
        artist: None,
        source: None,
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: true,
        up: None,
        mid: None,
        no_filter: false,
    };

    run_online_capture(&options, &online, &library);

    assert_eq!(online.calls()[0].1, ONLINE_SEARCH_PER_SOURCE);
}

fn up_options(keyword: &str, up: Option<&str>, mid: Option<u64>) -> SearchOptions {
    SearchOptions {
        keyword: keyword.to_owned(),
        artist: None,
        source: None,
        limit: DEFAULT_SEARCH_RESULTS,
        local: false,
        explicit_limit: false,
        has_keyword: !keyword.is_empty(),
        up: up.map(str::to_owned),
        mid,
        no_filter: false,
    }
}

fn up_track() -> SourceTrack {
    let mut track = online_track(SourceId::Bilibili, "BV1rG411n7qp", "最长的电影");
    track.artist = "明前奶绿".to_owned();
    track.duration_ms = 236_000;
    track
}

fn run_up_capture(options: &SearchOptions, online: &FakeUpOnline) -> Block {
    let library = FakeLibraryApi::new();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    run_up_search(
        options,
        online,
        &library,
        &emit,
        &running_template(),
        &|| 2_000,
    );
    emitted
        .lock()
        .expect("emitted")
        .last()
        .expect("done")
        .clone()
}

#[test]
fn up_option_conflicts_are_arg_errors() {
    for input in [
        "search --up 明前奶绿 --artist 周杰伦",
        "search --up 明前奶绿 --local",
        "search --up 明前奶绿 --source netease",
        "search --up 明前奶绿 --mid 2132180406",
        "search --mid abc",
        "search --mid 0",
    ] {
        let error = parse(input).expect_err(input);
        assert_eq!(error.code.as_str(), "E_CMD_ARG", "{input}");
    }
}

#[test]
fn up_search_prints_matched_user_and_results() {
    let online = FakeUpOnline::new(
        Ok(UpResolution {
            mid: 2132180406,
            name: Some("明前奶绿".to_owned()),
            fans: 310066,
            same_name: 2,
        }),
        Ok(vec![up_track()]),
    );
    let options = up_options("最长的电影", Some("明前奶绿"), None);

    let block = run_up_capture(&options, &online);

    let text = text_of_block(&block);
    assert!(text.contains("已匹配 UP：明前奶绿（mid=2132180406，粉丝 31.0 万）"));
    assert!(block.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. } if text.contains("另有 1 个同名用户")
    )));
    assert_eq!(table_of_block(&block).1.len(), 1);
    assert!(block.footer.as_deref().unwrap().contains("共 1 条"));
    assert_eq!(online.resolve_calls(), 1);
    assert_eq!(online.archive_calls()[0].0, 2132180406);
}

#[test]
fn up_mid_skips_user_resolution() {
    let online = FakeUpOnline::new(
        Err(CmdError::new("E_SRC_UNAVAILABLE", "不应调用")),
        Ok(vec![up_track()]),
    );
    let options = up_options("", None, Some(2132180406));

    let block = run_up_capture(&options, &online);

    assert_eq!(online.resolve_calls(), 0);
    assert!(text_of_block(&block).contains("已指定 UP：mid=2132180406"));
    assert!(online.archive_calls()[0].1.starts_with("UP 2132180406"));
    assert_eq!(online.archive_calls()[0].3, UP_ARCHIVE_PAGE_SIZE);
}

#[test]
fn up_no_keyword_requests_thirty_and_notes_recent() {
    let online = FakeUpOnline::new(
        Ok(UpResolution {
            mid: 1,
            name: Some("UP".to_owned()),
            fans: 0,
            same_name: 0,
        }),
        Ok(vec![up_track()]),
    );
    let options = up_options("", Some("UP"), None);

    let block = run_up_capture(&options, &online);

    assert_eq!(online.archive_calls()[0].3, 30);
    assert!(
        block
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("只显示最近 30 条"))
    );
}

#[test]
fn up_keyword_uses_requested_limit() {
    let online = FakeUpOnline::new(
        Ok(UpResolution {
            mid: 1,
            name: Some("UP".to_owned()),
            fans: 0,
            same_name: 0,
        }),
        Ok(vec![up_track()]),
    );
    let mut options = up_options("歌", Some("UP"), None);
    options.explicit_limit = true;
    options.limit = 5;

    run_up_capture(&options, &online);

    assert_eq!(online.archive_calls()[0].3, 5);
}

#[test]
fn up_rate_limit_is_human_error() {
    let online = FakeUpOnline::new(
        Ok(UpResolution {
            mid: 1,
            name: Some("UP".to_owned()),
            fans: 0,
            same_name: 0,
        }),
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "B 站限流，请稍后再试（未自动重试）",
        )),
    );
    let options = up_options("歌", Some("UP"), None);

    let block = run_up_capture(&options, &online);

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("B 站限流"));
    assert!(error_fields(&block).1.contains("未自动重试"));
}

#[test]
fn up_resolution_failure_is_error() {
    let online = FakeUpOnline::new(
        Err(CmdError::new("E_SRC_UNAVAILABLE", "没有找到 UP：nope")),
        Ok(Vec::new()),
    );
    let options = up_options("", Some("nope"), None);

    let block = run_up_capture(&options, &online);

    assert_eq!(block.status, BlockStatus::Error);
    assert!(error_fields(&block).1.contains("没有找到 UP"));
    assert!(online.archive_calls().is_empty());
}

#[test]
fn run_online_search_empty_hits_sets_empty_search_listing() {
    let library = FakeLibraryApi::new();
    let online = FakeOnline {
        tracks: Vec::new(),
        all_failed: false,
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    let template = running_template();

    run_online_search(
        "nothing",
        MAX_SEARCH_RESULTS,
        &online,
        &library,
        &emit,
        &template,
        &|| 2_000,
    );

    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Search,
            len: 0,
        })
    );
}

#[test]
fn online_prepare_downloads_and_reports_progress() {
    let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, vec![1, 2, 3]));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let track = online_track(SourceId::Netease, "10001", "Song");

    let mut seen = Vec::new();
    let prepared = service
        .prepare(&track, &mut |written| seen.push(written))
        .expect("下载成功");

    assert!(!prepared.cached);
    assert_eq!(prepared.bytes, 3);
    assert_eq!(fs::read(&prepared.path).expect("读取缓存"), b"abc");
    assert_eq!(seen, vec![1, 2, 3]);
    assert_eq!(source.resolves(), 1);
    assert_eq!(downloader.calls(), 1);
}

#[test]
fn online_prepare_hits_cache_without_resolve() {
    let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let track = online_track(SourceId::Netease, "10001", "Song");
    service.prepare(&track, &mut |_| {}).expect("首次下载");

    let prepared = service.prepare(&track, &mut |_| {}).expect("命中缓存");

    assert!(prepared.cached);
    assert_eq!(prepared.bytes, 3);
    assert_eq!(source.resolves(), 1, "命中缓存不应 resolve");
    assert_eq!(downloader.calls(), 1, "命中缓存不应下载");
    assert!(
        service.track_by_path(&prepared.path).is_some(),
        "命中缓存也要保证元数据已登记"
    );
}

#[test]
fn online_prepare_retries_expired_url_once() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, vec![ResolvePlan::Expired403]);
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

    let prepared = service
        .prepare(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
            &mut |_| {},
        )
        .expect("第二次 resolve 应成功");

    assert!(!prepared.cached);
    assert_eq!(source.resolves(), 2);
}

#[test]
fn online_prepare_gives_up_after_two_expired_urls() {
    let source = FakeOnlineSource::new(
        SourceId::Bilibili,
        vec![ResolvePlan::Expired403, ResolvePlan::Expired410],
    );
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

    let error = service
        .prepare(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
            &mut |_| {},
        )
        .expect_err("两次过期应失败");

    assert_eq!(error.code, "E_SRC_UNAVAILABLE");
    assert_eq!(error.message, "链接已过期，请重新搜索");
    assert_eq!(source.resolves(), 2);
}

#[test]
fn online_prepare_maps_copyright_to_human_message() {
    let source = FakeOnlineSource::new(SourceId::Netease, vec![ResolvePlan::Copyright]);
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

    let error = service
        .prepare(
            &online_track(SourceId::Netease, "10002", "VIP"),
            &mut |_| {},
        )
        .expect_err("版权受限");

    assert_eq!(error.code, "E_SRC_UNAVAILABLE");
    assert!(error.message.contains("网易云不可播放"));
    assert!(error.message.contains("版权受限") || error.message.contains("会员"));
}

#[test]
fn online_prepare_maps_download_failure() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", true, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

    let error = service
        .prepare(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
            &mut |_| {},
        )
        .expect_err("下载失败");

    assert_eq!(error.code, "E_SRC_DOWNLOAD");
    assert_eq!(error.message, "下载失败（已清理临时文件）");
}

#[test]
fn online_prepare_maps_network_error() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, vec![ResolvePlan::Network]);
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

    let error = service
        .prepare(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
            &mut |_| {},
        )
        .expect_err("网络错误");

    assert_eq!(error.code, "E_NET_TIMEOUT");
    assert!(error.message.contains("网络超时"));
}

#[test]
fn online_prepare_dedups_concurrent_same_track() {
    let source =
        FakeOnlineSource::new(SourceId::Netease, Vec::new()).with_delay(Duration::from_millis(50));
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let service = Arc::new(service);
    let track = online_track(SourceId::Netease, "10001", "Song");

    let handles: Vec<_> = (0..2)
        .map(|_| {
            let service = Arc::clone(&service);
            let track = track.clone();
            std::thread::spawn(move || service.prepare(&track, &mut |_| {}))
        })
        .collect();
    for handle in handles {
        handle.join().expect("thread").expect("prepare");
    }

    assert_eq!(source.resolves(), 1, "同曲目并发只 resolve 一次");
    assert_eq!(downloader.calls(), 1, "同曲目并发只下载一次");
}

#[test]
fn search_outcome_numbers_hits_across_sources() {
    let local = track(1, "Local", "Artist", "/music/local.flac", 1_000);
    let hits = vec![
        SearchHit::Local(local),
        SearchHit::Online(online_track(SourceId::Bilibili, "BV1xx", "Bili")),
        SearchHit::Online(online_track(SourceId::Netease, "10001", "Net")),
    ];

    let outcome = build_search_outcome("测试", &hits, false);
    let table = outcome
        .outputs
        .iter()
        .find_map(|output| match output {
            Output::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("table");

    assert_eq!(table.len(), 3);
    assert_eq!(table[0][0], "1");
    assert_eq!(table[0][1], "本地");
    assert_eq!(table[1][0], "2");
    assert_eq!(table[1][1], "B站");
    assert_eq!(table[2][0], "3");
    assert_eq!(table[2][1], "网易云");
    assert!(
        outcome
            .footer
            .as_deref()
            .is_some_and(|footer| footer.contains("共 3 条"))
    );
}

#[test]
fn search_outcome_all_online_failed_adds_hint() {
    let hits = vec![SearchHit::Local(track(
        1,
        "Local",
        "Artist",
        "/music/local.flac",
        1_000,
    ))];

    let outcome = build_search_outcome("测试", &hits, true);

    assert!(outcome.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. } if text == "在线源暂时不可用"
    )));
}

#[test]
fn search_outcome_empty_is_an_empty_table_with_hint() {
    let outcome = build_search_outcome("没有结果", &[], false);

    let table = outcome
        .outputs
        .iter()
        .find_map(|output| match output {
            Output::Table { rows, .. } => Some(rows),
            _ => None,
        })
        .expect("empty table");
    assert!(table.is_empty());
    assert!(outcome.outputs.iter().any(|output| matches!(
        output,
        Output::Hint { text, .. } if text == "换个关键词试试"
    )));
}

struct FakeOnline {
    tracks: Vec<SourceTrack>,
    all_failed: bool,
}

impl OnlineApi for FakeOnline {
    fn known_sources(&self) -> Vec<SourceId> {
        KNOWN_SOURCES.to_vec()
    }

    fn set_source_enabled(&self, _source: SourceId, _enabled: bool) -> Result<(), CmdError> {
        Ok(())
    }

    fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
        Ok(OnlineSearch {
            tracks: self.tracks.clone(),
            all_failed: self.all_failed,
        })
    }

    fn prepare(
        &self,
        _track: &SourceTrack,
        _progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
    }
}

struct FakeUpOnline {
    resolution: Result<UpResolution, CmdError>,
    archives: Result<Vec<SourceTrack>, CmdError>,
    resolve_calls: AtomicUsize,
    archive_calls: Mutex<Vec<(u64, String, String, usize)>>,
}

impl FakeUpOnline {
    fn new(
        resolution: Result<UpResolution, CmdError>,
        archives: Result<Vec<SourceTrack>, CmdError>,
    ) -> Self {
        Self {
            resolution,
            archives,
            resolve_calls: AtomicUsize::new(0),
            archive_calls: Mutex::new(Vec::new()),
        }
    }

    fn resolve_calls(&self) -> usize {
        self.resolve_calls.load(Ordering::SeqCst)
    }

    fn archive_calls(&self) -> Vec<(u64, String, String, usize)> {
        self.archive_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl OnlineApi for FakeUpOnline {
    fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
        Ok(OnlineSearch {
            tracks: Vec::new(),
            all_failed: false,
        })
    }

    fn prepare(
        &self,
        _track: &SourceTrack,
        _progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
    }

    fn resolve_up(&self, _name: &str) -> Result<UpResolution, CmdError> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        self.resolution.clone()
    }

    fn up_archives(
        &self,
        mid: u64,
        name: &str,
        keyword: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, CmdError> {
        self.archive_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((mid, name.to_owned(), keyword.to_owned(), limit));
        self.archives.clone()
    }
}

struct ChannelOnline {
    by_query: HashMap<String, Vec<SourceTrack>>,
    resolution: Result<UpResolution, CmdError>,
    archives: Result<Vec<SourceTrack>, CmdError>,
    calls: Mutex<Vec<(String, Option<SourceId>)>>,
    resolve_calls: AtomicUsize,
    archive_calls: AtomicUsize,
}

impl ChannelOnline {
    fn new(
        by_query: HashMap<String, Vec<SourceTrack>>,
        resolution: Result<UpResolution, CmdError>,
        archives: Result<Vec<SourceTrack>, CmdError>,
    ) -> Self {
        Self {
            by_query,
            resolution,
            archives,
            calls: Mutex::new(Vec::new()),
            resolve_calls: AtomicUsize::new(0),
            archive_calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> Vec<(String, Option<SourceId>)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl OnlineApi for ChannelOnline {
    fn search_all(&self, query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
        self.search_all_for(query, _limit, None)
    }

    fn search_all_for(
        &self,
        query: &str,
        _limit: usize,
        source: Option<SourceId>,
    ) -> Result<OnlineSearch, CmdError> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((query.to_owned(), source));
        Ok(OnlineSearch {
            tracks: self.by_query.get(query).cloned().unwrap_or_default(),
            all_failed: false,
        })
    }

    fn prepare(
        &self,
        _track: &SourceTrack,
        _progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
    }

    fn resolve_up(&self, _name: &str) -> Result<UpResolution, CmdError> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        self.resolution.clone()
    }

    fn up_archives(
        &self,
        _mid: u64,
        _name: &str,
        _keyword: &str,
        _limit: usize,
    ) -> Result<Vec<SourceTrack>, CmdError> {
        self.archive_calls.fetch_add(1, Ordering::SeqCst);
        self.archives.clone()
    }
}

struct RecordingOnline {
    tracks: Vec<SourceTrack>,
    calls: Mutex<Vec<(String, usize, Option<SourceId>)>>,
}

impl RecordingOnline {
    fn new(tracks: Vec<SourceTrack>) -> Self {
        Self {
            tracks,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, usize, Option<SourceId>)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl OnlineApi for RecordingOnline {
    fn search_all(&self, query: &str, limit: usize) -> Result<OnlineSearch, CmdError> {
        self.search_all_for(query, limit, None)
    }

    fn search_all_for(
        &self,
        query: &str,
        limit: usize,
        source: Option<SourceId>,
    ) -> Result<OnlineSearch, CmdError> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((query.to_owned(), limit, source));
        let mut tracks = self.tracks.clone();
        if let Some(source) = source {
            tracks.retain(|track| track.source == source);
        }
        Ok(OnlineSearch {
            tracks,
            all_failed: false,
        })
    }

    fn prepare(
        &self,
        _track: &SourceTrack,
        _progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
    }
}

struct FakeQueueOnline {
    path: PathBuf,
}

impl OnlineApi for FakeQueueOnline {
    fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
        Ok(OnlineSearch {
            tracks: Vec::new(),
            all_failed: false,
        })
    }

    fn prepare(
        &self,
        _track: &SourceTrack,
        _progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        Ok(PreparedTrack {
            path: self.path.clone(),
            bytes: 4,
            cached: false,
        })
    }
}

fn running_template() -> Block {
    Block {
        id: "online-block".to_owned(),
        input: "play 1".to_owned(),
        started_at: 1_000,
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: Vec::new(),
        footer: None,
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

#[test]
fn run_online_search_merges_local_and_online_hits() {
    let library = FakeLibraryApi::new().with_search_tracks(vec![track(
        1,
        "Local",
        "Artist",
        "/music/local.flac",
        1_000,
    )]);
    let online = FakeOnline {
        tracks: vec![online_track(SourceId::Bilibili, "BV1xx", "Bili")],
        all_failed: false,
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    let template = running_template();

    run_online_search(
        "测试",
        MAX_SEARCH_RESULTS,
        &online,
        &library,
        &emit,
        &template,
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done block");
    assert_eq!(done.status, BlockStatus::Done);
    let rows = done
        .outputs
        .iter()
        .find_map(|output| match output {
            Output::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("table");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][1], "本地");
    assert_eq!(rows[1][1], "B站");
    assert_eq!(library.hits().len(), 2);
    assert_eq!(
        library.active_listing(),
        Some(ActiveListing {
            source: ListingSource::Search,
            len: 2,
        })
    );
}

struct FakePlayback {
    paths: Mutex<Vec<String>>,
    fail: bool,
}

impl FakePlayback {
    fn new() -> Self {
        Self {
            paths: Mutex::new(Vec::new()),
            fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            paths: Mutex::new(Vec::new()),
            fail: true,
        }
    }

    fn paths(&self) -> Vec<String> {
        self.paths.lock().expect("paths mutex").clone()
    }
}

impl PlaybackTarget for FakePlayback {
    fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError> {
        self.paths
            .lock()
            .expect("paths mutex")
            .push(path.to_owned());
        if self.fail {
            return Err(CmdError::new("E_DECODE", "无法解码"));
        }
        Ok(PlaybackState::default())
    }
}

#[test]
fn run_online_play_downloads_then_plays_with_footer() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let playback = FakePlayback::new();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    let template = running_template();

    run_online_play(
        &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
        0,
        &service,
        &playback,
        &emit,
        &template,
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    assert_eq!(done.status, BlockStatus::Done);
    assert!(
        done.footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：B站") && footer.contains("已缓存"))
    );
    assert_eq!(playback.paths().len(), 1);
}

#[test]
fn run_online_play_cached_footer_says_hit() {
    let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let track = online_track(SourceId::Netease, "10001", "Song");
    service.prepare(&track, &mut |_| {}).expect("预热缓存");
    let playback = FakePlayback::new();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_play(
        &track,
        0,
        &service,
        &playback,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let done = blocks.last().expect("done");
    assert_eq!(done.status, BlockStatus::Done);
    assert!(
        done.footer
            .as_deref()
            .is_some_and(|footer| footer.contains("来源：网易云") && footer.contains("命中缓存"))
    );
}

#[test]
fn run_online_play_error_block_uses_mapped_code() {
    let online = FakeOnline {
        tracks: Vec::new(),
        all_failed: false,
    };
    struct FailingOnline;
    impl OnlineApi for FailingOnline {
        fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
            Ok(OnlineSearch {
                tracks: Vec::new(),
                all_failed: false,
            })
        }

        fn prepare(
            &self,
            _track: &SourceTrack,
            _progress: &mut dyn FnMut(u64),
        ) -> Result<PreparedTrack, CmdError> {
            Err(CmdError::new("E_SRC_UNAVAILABLE", "版权受限"))
        }
    }
    let playback = FakePlayback::new();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_play(
        &online_track(SourceId::Netease, "10002", "VIP"),
        0,
        &FailingOnline,
        &playback,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let error = blocks.last().expect("error block");
    assert_eq!(error.status, BlockStatus::Error);
    assert_eq!(error_fields(error).0, "E_SRC_UNAVAILABLE");
    assert!(playback.paths().is_empty());
    let _ = online;
}

#[test]
fn run_online_play_progress_is_time_throttled() {
    struct ProgressOnline {
        clock: Arc<AtomicU64>,
        values: Vec<(u64, u64)>,
        path: PathBuf,
    }
    impl OnlineApi for ProgressOnline {
        fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
            Ok(OnlineSearch {
                tracks: Vec::new(),
                all_failed: false,
            })
        }

        fn prepare(
            &self,
            _track: &SourceTrack,
            progress: &mut dyn FnMut(u64),
        ) -> Result<PreparedTrack, CmdError> {
            for (at, written) in &self.values {
                self.clock.store(*at, Ordering::SeqCst);
                progress(*written);
            }
            Ok(PreparedTrack {
                path: self.path.clone(),
                bytes: 4,
                cached: false,
            })
        }
    }

    let clock = Arc::new(AtomicU64::new(1_000));
    let dir = TestDir::new("progress");
    let path = dir.path().join("audio.mp3");
    fs::write(&path, b"abc").expect("write");
    let online = ProgressOnline {
        clock: Arc::clone(&clock),
        values: vec![(1_000, 1), (1_200, 2), (1_600, 3), (2_200, 4)],
        path,
    };
    let playback = FakePlayback::new();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };
    let clock_for_run = Arc::clone(&clock);

    run_online_play(
        &online_track(SourceId::Bilibili, "BV1xx", "Video"),
        0,
        &online,
        &playback,
        &emit,
        &running_template(),
        &move || clock_for_run.load(Ordering::SeqCst),
    );

    let blocks = emitted.lock().expect("emitted");
    let progress_blocks = blocks
        .iter()
        .filter(|block| {
            block
                .outputs
                .iter()
                .any(|output| matches!(output, Output::Progress { .. }))
        })
        .count();
    assert_eq!(progress_blocks, 3, "500ms 节流：1000/1600/2200 各一次");
}

#[test]
fn run_online_play_playback_failure_emits_error() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let playback = FakePlayback::failing();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let emit = {
        let emitted = Arc::clone(&emitted);
        move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
    };

    run_online_play(
        &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
        0,
        &service,
        &playback,
        &emit,
        &running_template(),
        &|| 2_000,
    );

    let blocks = emitted.lock().expect("emitted");
    let error = blocks.last().expect("error block");
    assert_eq!(error.status, BlockStatus::Error);
    assert_eq!(error_fields(error).0, "E_DECODE");
    assert_eq!(playback.paths().len(), 1);
}

#[test]
fn search_dispatch_only_goes_online_without_local_flag() {
    let online = parse("search 晴天").expect("valid").expect("command");
    let local = parse("search 晴天 --local")
        .expect("valid")
        .expect("command");

    assert!(search_is_online(&online));
    assert!(!search_is_online(&local));
}

#[test]
fn online_play_hit_requires_search_listing_and_online_track() {
    let hits = vec![
        SearchHit::Local(track(1, "Local", "Artist", "/music/a.flac", 1_000)),
        SearchHit::Online(online_track(SourceId::Bilibili, "BV1xx", "Bili")),
    ];
    let play_two = parse("play 2").expect("valid").expect("command");
    let play_one = parse("play 1").expect("valid").expect("command");
    let play_three = parse("play 3").expect("valid").expect("command");
    let search = ActiveListing {
        source: ListingSource::Search,
        len: 2,
    };
    let queue = ActiveListing {
        source: ListingSource::Queue,
        len: 2,
    };

    let hit = online_play_hit(&play_two, Some(&search), &hits).expect("在线结果");
    assert_eq!(hit.0, 1);
    assert_eq!(hit.1.source_id, "BV1xx");
    assert!(
        online_play_hit(&play_one, Some(&search), &hits).is_none(),
        "本地结果不截走"
    );
    assert!(
        online_play_hit(&play_two, Some(&queue), &hits).is_none(),
        "队列上下文不截走"
    );
    assert!(
        online_play_hit(&play_two, None, &hits).is_none(),
        "无上下文不截走"
    );
    assert!(
        online_play_hit(&play_three, Some(&search), &hits).is_none(),
        "越界"
    );
}

// ---- 远程曲目元数据（T-0039） ----

#[test]
fn remote_track_has_the_frontend_track_shape() {
    let dir = TestDir::new("remote-shape");
    let service = OnlineService::new(
        dir.path().to_path_buf(),
        KNOWN_SOURCES.iter().copied().collect(),
    );
    let path = dir.path().join("bilibili-BV1xx411c7mD.m4a");
    fs::write(&path, b"abc").expect("write cache");
    let source_track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "晴天");
    service.remember_remote(&path, &source_track);

    let track = service.track_by_path(&path).expect("远程命中");

    assert_eq!(track.id, 0);
    assert_eq!(track.kind, "remote");
    assert_eq!(track.path, path.to_string_lossy());
    assert_eq!(track.source, "bilibili");
    assert_eq!(track.source_id, "BV1xx411c7mD");
    assert_eq!(track.title, "晴天");
    assert_eq!(track.artist, "在线艺人");
    assert_eq!(track.duration_ms, 200_000);
    assert_eq!(track.album, "");
    assert_eq!(track.size_bytes, 3);
    assert_eq!(track.track_no, 0);
}

#[test]
fn lookup_track_prefers_local_and_skips_remote_map() {
    let dir = TestDir::new("remote-local-first");
    let service = OnlineService::new(
        dir.path().to_path_buf(),
        KNOWN_SOURCES.iter().copied().collect(),
    );
    let path = dir.path().join("bilibili-BV1xx.m4a");
    fs::write(&path, b"abc").expect("write cache");
    service.remember_remote(
        &path,
        &online_track(SourceId::Bilibili, "BV1xx", "远程标题"),
    );
    let local = track(1, "本地标题", "本地艺人", "/music/local.flac", 1_000);

    let found = lookup_track(Some(local.clone()), &service, &path);

    assert_eq!(found, Some(local));
    assert_eq!(service.track_lookups(), 0, "本地命中不应查远程映射");
}

#[test]
fn lookup_track_returns_none_when_both_sides_miss() {
    let dir = TestDir::new("remote-miss");
    let service = OnlineService::new(
        dir.path().to_path_buf(),
        KNOWN_SOURCES.iter().copied().collect(),
    );

    assert!(lookup_track(None, &service, &dir.path().join("nope.m4a")).is_none());
    assert_eq!(service.track_lookups(), 1);
}

#[test]
fn prepare_success_registers_remote_metadata() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let source_track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "晴天");

    let prepared = service.prepare(&source_track, &mut |_| {}).expect("下载");

    let track = service.track_by_path(&prepared.path).expect("已登记");
    assert_eq!(track.kind, "remote");
    assert_eq!(track.title, "晴天");
    assert_eq!(track.source_id, "BV1xx411c7mD");
    assert_eq!(service.remote_meta_len(), 1);
}

#[test]
fn prepare_failure_does_not_register_remote_metadata() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", true, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

    service
        .prepare(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "失败"),
            &mut |_| {},
        )
        .expect_err("下载失败");

    assert_eq!(service.remote_meta_len(), 0);
}

#[test]
fn missing_cache_file_removes_remote_metadata() {
    let dir = TestDir::new("remote-missing");
    let service = OnlineService::new(
        dir.path().to_path_buf(),
        KNOWN_SOURCES.iter().copied().collect(),
    );
    let path = dir.path().join("netease-10001.mp3");
    fs::write(&path, b"abc").expect("write cache");
    service.remember_remote(&path, &online_track(SourceId::Netease, "10001", "Song"));
    fs::remove_file(&path).expect("remove cache");

    assert!(service.track_by_path(&path).is_none());
    assert_eq!(service.remote_meta_len(), 0);
}

#[test]
fn prepare_prunes_metadata_for_files_removed_by_cache_cleanup() {
    let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
    let stale = _dir.path().join("bilibili-BVstale.m4a");
    fs::write(&stale, b"old").expect("write stale");
    service.remember_remote(&stale, &online_track(SourceId::Bilibili, "BVstale", "Old"));
    fs::remove_file(&stale).expect("模拟 LRU 清理");

    service
        .prepare(
            &online_track(SourceId::Bilibili, "BVnew", "New"),
            &mut |_| {},
        )
        .expect("新曲目下载");

    assert_eq!(service.remote_meta_len(), 1, "只应保留新下载的映射");
    assert!(service.track_by_path(&stale).is_none());
}

#[test]
fn library_track_by_path_falls_back_to_remote_metadata() {
    let dir = TestDir::new("remote-library");
    let handle = LibraryHandle::new(dir.path().join("library.db"));
    let path = dir.path().join("netease-10001.mp3");
    fs::write(&path, b"abc").expect("write cache");
    handle
        .online
        .remember_remote(&path, &online_track(SourceId::Netease, "10001", "Song"));

    let found = handle
        .track_by_path(path.to_str().expect("utf8"))
        .expect("查询")
        .expect("远程命中");

    assert_eq!(found.kind, "remote");
    assert_eq!(found.source, "netease");
    assert_eq!(found.title, "Song");
}

#[test]
fn cache_root_is_injected_from_setup() {
    let dir = TestDir::new("cache-root");
    let cache_root = dir.path().join("system-cache");
    let handle = LibraryHandle::with_cache_root(dir.path().join("library.db"), cache_root.clone());

    assert_eq!(handle.online.cache_root(), cache_root);
}

#[test]
fn shutdown_sets_cancel_flag_and_is_idempotent() {
    let dir = TestDir::new("shutdown-cancel");
    let handle = LibraryHandle::new(dir.path().join("library.db"));
    assert!(!handle.online.is_cancelled());

    handle.shutdown();
    handle.shutdown();

    assert!(handle.online.is_cancelled());
}

// ---- 设置命令族：theme（T-0042） ----

struct FakeTheme {
    entries: Vec<ThemeListEntry>,
    apply: Result<ThemeApplyReply, CmdError>,
    export: Result<ThemeExportTarget, CmdError>,
    broadcasts: Mutex<Vec<String>>,
}

impl FakeTheme {
    fn new(apply: Result<ThemeApplyReply, CmdError>) -> Self {
        Self {
            entries: vec![
                ThemeListEntry {
                    id: "graphite".to_owned(),
                    name: "石墨".to_owned(),
                    skin: np_core::theme::Skin::Plain,
                    source: ThemeSource::Builtin,
                },
                ThemeListEntry {
                    id: "mine".to_owned(),
                    name: "我的".to_owned(),
                    skin: np_core::theme::Skin::Phosphor,
                    source: ThemeSource::User,
                },
            ],
            apply,
            export: Ok(ThemeExportTarget {
                path: "/themes/ice.json".to_owned(),
                created: true,
            }),
            broadcasts: Mutex::new(Vec::new()),
        }
    }

    fn broadcasts(&self) -> Vec<String> {
        self.broadcasts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ThemeApi for FakeTheme {
    fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError> {
        Ok(self.entries.clone())
    }

    fn apply(&self, _id: &str) -> Result<ThemeApplyReply, CmdError> {
        self.apply.clone()
    }

    fn export(&self, _id: &str) -> Result<ThemeExportTarget, CmdError> {
        self.export.clone()
    }

    fn broadcast(&self, id: &str) {
        self.broadcasts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(id.to_owned());
    }
}

fn theme_reply(id: &str) -> ThemeApplyReply {
    ThemeApplyReply {
        id: id.to_owned(),
        name: "冰".to_owned(),
        skin: np_core::theme::Skin::Plain,
        source: ThemeSource::Builtin,
        tokens: np_core::theme::ThemeTokens::default(),
        structure: np_core::theme::Structure::default().resolve(np_core::theme::Skin::Plain),
        warnings: Vec::new(),
    }
}

fn settings_for(dir: &TestDir) -> SettingsHandle {
    SettingsHandle::load(dir.path().join("config"))
}

fn done_with_services(
    api: &FakeApi,
    library: &FakeLibraryApi,
    settings: &SettingsHandle,
    theme: &FakeTheme,
    input: &str,
) -> Block {
    let services = Services {
        settings: Some(settings),
        theme: Some(theme),
        online: None,
    };
    match execute_with_services(
        api,
        library,
        services,
        "block-services",
        input,
        Arc::new(NoopLibraryObserver),
    ) {
        Execution::Blocks { done, .. } => *done,
        Execution::Accepted { .. } => panic!("unexpected accepted block: {input}"),
        Execution::Ignored => panic!("expected a block: {input}"),
    }
}

#[test]
fn theme_set_writes_settings_and_broadcasts() {
    let dir = TestDir::new("theme-set");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_services(&api, &library, &settings, &theme, "theme set ice");

    assert_eq!(block.status, BlockStatus::Done, "{}", text_of_block(&block));
    assert_eq!(settings.theme_id().as_deref(), Some("ice"));
    assert_eq!(theme.broadcasts(), vec!["ice".to_owned()]);
    assert!(text_of_block(&block).contains("主题已切换"));
}

#[test]
fn theme_set_invalid_theme_does_not_write_or_broadcast() {
    let dir = TestDir::new("theme-set-bad");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme =
        FakeTheme::new(Err(CmdError::new("E_THEME_NOT_FOUND", "主题不存在")
            .with_hint("用 theme.list 查看可用主题")));

    let block = done_with_services(&api, &library, &settings, &theme, "theme set nope");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_THEME_NOT_FOUND");
    let (_, _, hint) = error_fields(&block);
    let hint = hint.expect("hint");
    assert!(hint.contains("theme list"), "{hint}");
    assert!(!hint.contains("theme.list"), "{hint}");
    assert_eq!(settings.theme_id(), None);
    assert!(theme.broadcasts().is_empty());
}

#[test]
fn theme_list_shows_tui_structure_summary() {
    let dir = TestDir::new("theme-list-tui");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme {
        entries: vec![
            ThemeListEntry {
                id: "tui-block".to_owned(),
                name: "TUI 块面".to_owned(),
                skin: np_core::theme::Skin::Plain,
                source: ThemeSource::Builtin,
            },
            ThemeListEntry {
                id: "tui-wire".to_owned(),
                name: "TUI 线框".to_owned(),
                skin: np_core::theme::Skin::Plain,
                source: ThemeSource::Builtin,
            },
            ThemeListEntry {
                id: "tui-paper".to_owned(),
                name: "TUI 票据".to_owned(),
                skin: np_core::theme::Skin::Plain,
                source: ThemeSource::Builtin,
            },
        ],
        apply: Ok(theme_reply("tui-block")),
        export: Ok(ThemeExportTarget {
            path: "/themes/tui-block.json".to_owned(),
            created: true,
        }),
        broadcasts: Mutex::new(Vec::new()),
    };

    let block = done_with_services(&api, &library, &settings, &theme, "theme list");
    let (columns, rows) = table_of_block(&block);
    assert_eq!(columns.last().copied(), Some("结构"));
    assert_eq!(rows[0][5], "块面/紧凑/反白");
    assert_eq!(rows[1][5], "线框/紧凑/底色");
    assert_eq!(rows[2][5], "票据/常规/反白");
}

#[test]
fn theme_list_marks_current_and_sources() {
    let dir = TestDir::new("theme-list");
    let settings = settings_for(&dir);
    settings
        .set_and_save("theme.id", SettingValue::Str("mine".into()))
        .expect("save theme");
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_services(&api, &library, &settings, &theme, "theme list");

    let (_, rows) = table_of_block(&block);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "");
    assert_eq!(rows[0][1], "graphite");
    assert_eq!(rows[0][3], "内置");
    assert_eq!(rows[0][5], "圆角/常规/侧条");
    assert_eq!(rows[1][0], "●");
    assert_eq!(rows[1][1], "mine");
    assert_eq!(rows[1][3], "用户");
    assert_eq!(rows[1][4], "phosphor");
    assert_eq!(rows[1][5], "自定义");
}

#[test]
fn theme_export_reports_path_and_created() {
    let dir = TestDir::new("theme-export");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_services(&api, &library, &settings, &theme, "theme export ice");

    assert_eq!(block.status, BlockStatus::Done);
    assert!(text_of_block(&block).contains("/themes/ice.json"));
    assert!(text_of_block(&block).contains("已导出"));
}

fn done_with_online_services(
    api: &FakeApi,
    library: &FakeLibraryApi,
    settings: &SettingsHandle,
    theme: &FakeTheme,
    online: &dyn OnlineApi,
    input: &str,
) -> Block {
    let services = Services {
        settings: Some(settings),
        theme: Some(theme),
        online: Some(online),
    };
    match execute_with_services(
        api,
        library,
        services,
        "block-online-services",
        input,
        Arc::new(NoopLibraryObserver),
    ) {
        Execution::Blocks { done, .. } => *done,
        Execution::Accepted { .. } => panic!("unexpected accepted block: {input}"),
        Execution::Ignored => panic!("expected a block: {input}"),
    }
}

#[test]
fn config_set_infers_types_and_lists_sorted() {
    let dir = TestDir::new("config-set");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set theme.id mine",
    );
    done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set playback.crossfadeMs 300",
    );
    done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set ui.compact true",
    );

    let entries = settings.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].0, "playback.crossfadeMs");
    assert_eq!(entries[0].1, SettingValue::Num(300.0));
    assert_eq!(entries[1].0, "theme.id");
    assert_eq!(entries[2].1, SettingValue::Bool(true));

    let block = done_with_services(&api, &library, &settings, &theme, "config list");
    let text = text_of_block(&block);
    assert!(text.contains("playback.crossfadeMs = 300"));
    assert!(text.contains("theme.id = mine"));
}

#[test]
fn config_get_missing_is_arg_error() {
    let dir = TestDir::new("config-get-missing");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_services(&api, &library, &settings, &theme, "config get theme.id");

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(error_fields(&block).1.contains("未设置"));
}

#[test]
fn config_unset_and_reset() {
    let dir = TestDir::new("config-reset");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));
    done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set ui.compact true",
    );
    done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set theme.id mine",
    );

    let unset = done_with_services(&api, &library, &settings, &theme, "config unset ui.compact");
    assert!(text_of_block(&unset).contains("已移除"));
    assert!(settings.get("ui.compact").is_none());

    let reset = done_with_services(&api, &library, &settings, &theme, "config reset");
    assert_eq!(reset.status, BlockStatus::Done);
    assert!(settings.entries().is_empty());
}

#[test]
fn config_theme_id_requires_known_theme() {
    let dir = TestDir::new("config-theme-validate");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let ok = done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set theme.id mine",
    );
    assert_eq!(ok.status, BlockStatus::Done, "{}", text_of_block(&ok));
    assert_eq!(settings.theme_id().as_deref(), Some("mine"));

    let bad = done_with_services(
        &api,
        &library,
        &settings,
        &theme,
        "config set theme.id nope",
    );
    assert_eq!(bad.status, BlockStatus::Error);
    assert_eq!(settings.theme_id().as_deref(), Some("mine"));
}

#[test]
fn config_source_enabled_requires_known_source_and_bool() {
    let dir = TestDir::new("config-source-validate");
    let settings = settings_for(&dir);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));
    let online = FakeOnline {
        tracks: Vec::new(),
        all_failed: false,
    };

    let ok = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &online,
        "config set source.bilibili.enabled false",
    );
    assert_eq!(ok.status, BlockStatus::Done, "{}", text_of_block(&ok));

    let bad = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &online,
        "config set source.youtube.enabled false",
    );
    assert_eq!(bad.status, BlockStatus::Error);
    assert_eq!(settings.get_bool("source.bilibili.enabled"), Some(false));
}

fn two_source_service(
    dir: &TestDir,
    bili: Arc<FakeOnlineSource>,
    netease: Arc<FakeOnlineSource>,
) -> OnlineService {
    let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
    let cache = StreamCache::with_limits(
        dir.path(),
        downloader as Arc<dyn Downloader>,
        10 * 1024 * 1024,
        5 * 1024 * 1024,
    );
    let registry = SourceRegistry::new(vec![
        bili as Arc<dyn AudioSource>,
        netease as Arc<dyn AudioSource>,
    ]);
    OnlineService::from_parts(registry, cache)
}

#[test]
fn source_list_reads_enabled_state_from_settings() {
    let dir = TestDir::new("source-list");
    let settings = settings_for(&dir);
    settings
        .set_and_save("source.bilibili.enabled", SettingValue::Bool(false))
        .expect("save");
    let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let service = two_source_service(&dir, bili, netease);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block =
        done_with_online_services(&api, &library, &settings, &theme, &service, "source list");

    let (_, rows) = table_of_block(&block);
    let bili_row = rows.iter().find(|row| row[0] == "bilibili").expect("bili");
    let netease_row = rows
        .iter()
        .find(|row| row[0] == "netease")
        .expect("netease");
    assert_eq!(bili_row[2], "禁用");
    assert_eq!(netease_row[2], "启用");
}

#[test]
fn source_disable_writes_settings_and_blocks_old_result_resolve() {
    let dir = TestDir::new("source-disable");
    let settings = settings_for(&dir);
    let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let service = two_source_service(&dir, Arc::clone(&bili), netease);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &service,
        "source disable bilibili",
    );

    assert_eq!(block.status, BlockStatus::Done, "{}", text_of_block(&block));
    assert_eq!(settings.get_bool("source.bilibili.enabled"), Some(false));
    assert_eq!(service.known_sources().len(), 2, "禁用不删列表");
    let error = service
        .prepare(
            &online_track(SourceId::Bilibili, "BV1xx", "Old"),
            &mut |_| {},
        )
        .expect_err("禁用后旧结果应报错");
    assert_eq!(error.code, "E_SRC_UNAVAILABLE");
    assert!(error.message.contains("禁用"));
    assert_eq!(bili.resolves(), 0, "禁用源不应再 resolve");
}

#[test]
fn disabled_source_is_skipped_in_search() {
    let dir = TestDir::new("source-search-skip");
    let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let service = two_source_service(&dir, Arc::clone(&bili), Arc::clone(&netease));
    service
        .set_source_enabled(SourceId::Bilibili, false)
        .expect("disable");

    service.search_all("测试", 5).expect("search");

    assert_eq!(bili.searches(), 0);
    assert_eq!(netease.searches(), 1);
}

#[test]
fn source_enable_restores_settings_and_service() {
    let dir = TestDir::new("source-enable");
    let settings = settings_for(&dir);
    let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let service = two_source_service(&dir, bili, netease);
    service
        .set_source_enabled(SourceId::Bilibili, false)
        .expect("disable");
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &service,
        "source enable bilibili",
    );

    assert_eq!(block.status, BlockStatus::Done);
    assert_eq!(settings.get_bool("source.bilibili.enabled"), Some(true));
}

#[test]
fn source_unknown_id_is_arg_error_without_side_effects() {
    let dir = TestDir::new("source-unknown");
    let settings = settings_for(&dir);
    let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    let service = two_source_service(&dir, bili, netease);
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &service,
        "source disable youtube",
    );

    assert_eq!(block.status, BlockStatus::Error);
    assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    assert!(settings.entries().is_empty());
}

fn cache_service(dir: &TestDir) -> OnlineService {
    let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
    let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
    two_source_service(dir, bili, netease)
}

#[test]
fn purge_legacy_covers_removes_old_entries_without_output() {
    let dir = TestDir::new("purge-covers");
    let covers = dir.path().join("covers");
    fs::create_dir_all(covers.join("nested")).expect("covers dir");
    fs::write(covers.join("a.jpg"), b"abc").expect("cover file");
    fs::write(covers.join("nested/b.jpg"), b"abc").expect("nested cover");

    assert_eq!(purge_legacy_covers(&covers), 2);
    assert!(covers.is_dir());
    assert!(fs::read_dir(&covers).expect("read").next().is_none());
}

#[test]
fn cache_clear_streams_leaves_unmanaged_dirs() {
    let dir = TestDir::new("cache-clear-streams");
    let settings = settings_for(&dir);
    let service = cache_service(&dir);
    fs::create_dir_all(dir.path().join("streams")).expect("streams");
    fs::create_dir_all(dir.path().join("covers")).expect("covers");
    fs::write(dir.path().join("streams/a.mp3"), b"abcd").expect("streams file");
    fs::write(dir.path().join("covers/b.jpg"), b"abcdef").expect("covers file");
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &service,
        "cache clear streams",
    );

    assert_eq!(block.status, BlockStatus::Done);
    assert!(!dir.path().join("streams/a.mp3").exists());
    assert!(dir.path().join("covers/b.jpg").exists());
    assert!(text_of_block(&block).contains("释放 4 B"));
}

#[test]
fn cache_clear_all_stops_remote_playback_first() {
    let dir = TestDir::new("cache-clear-remote");
    let settings = settings_for(&dir);
    let service = cache_service(&dir);
    let path = dir.path().join("streams/bilibili-BV1xx.mp3");
    fs::create_dir_all(dir.path().join("streams")).expect("streams");
    fs::write(&path, b"abcd").expect("cache file");
    service.remember_remote(&path, &online_track(SourceId::Bilibili, "BV1xx", "Song"));
    let path_text = path.to_string_lossy().into_owned();
    let remote = service.track_by_path(&path).expect("remote meta");
    let api = FakeApi::new()
        .with_state(PlaybackState {
            track_id: Some(path_text.clone()),
            ..PlaybackState::default()
        })
        .with_queue(queue_view(&[&path_text], Some(0)));
    let library = FakeLibraryApi::new().with_track_by_path(&path_text, remote);
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &service,
        "cache clear all",
    );

    assert_eq!(block.status, BlockStatus::Done);
    assert!(api.calls().contains(&Call::QueueRemove(0)));
    assert!(text_of_block(&block).contains("已先停止播放远程曲目"));
    assert!(!path.exists());
}

#[test]
fn cache_clear_does_not_stop_local_playback() {
    let dir = TestDir::new("cache-clear-local");
    let settings = settings_for(&dir);
    let service = cache_service(&dir);
    let api = FakeApi::new()
        .with_state(PlaybackState {
            track_id: Some("/music/local.flac".to_owned()),
            ..PlaybackState::default()
        })
        .with_queue(queue_view(&["/music/local.flac"], Some(0)));
    let local = track(1, "Local", "Artist", "/music/local.flac", 1_000);
    let library = FakeLibraryApi::new().with_track_by_path("/music/local.flac", local);
    let theme = FakeTheme::new(Ok(theme_reply("ice")));

    let block = done_with_online_services(
        &api,
        &library,
        &settings,
        &theme,
        &service,
        "cache clear streams",
    );

    assert_eq!(block.status, BlockStatus::Done);
    assert!(!api.calls().contains(&Call::QueueRemove(0)));
    assert!(!text_of_block(&block).contains("已先停止"));
}

#[test]
fn help_lists_the_new_settings_families() {
    let api = FakeApi::new();
    let library = FakeLibraryApi::new();
    let block = done_with_library(&api, &library, "help");
    let text = text_of_block(&block);

    for family in ["theme", "config", "source", "cache"] {
        assert!(text.contains(family), "help 缺少 {family}: {text}");
    }
    assert!(block.outputs.len() >= 18);
}
