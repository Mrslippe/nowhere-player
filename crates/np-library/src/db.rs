//! 连接管理与迁移。
//!
//! 一个 [`Database`] 持有一条连接；扫描器的 writer 在调用线程上复用这条连接，
//! 因此任意时刻只有一个线程写库（见 `assets` 之外的 ARCH-003 §0）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::error::Result;
use crate::repo::{self, Stats, Track};
use crate::schema;

/// 每多少条提交一次（ARCH-003 §0 / 任务卡 §6.2）。
pub(crate) const BATCH_SIZE: usize = 200;

/// 媒体库数据库。
pub struct Database {
    conn: Connection,
    path: Option<PathBuf>,
}

impl Database {
    /// 打开（必要时创建）数据库并迁移到最新 schema。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let db = Self {
            conn: Connection::open(path)?,
            path: Some(path.to_path_buf()),
        };
        db.configure()?;
        db.migrate()?;
        Ok(db)
    }

    /// 打开一个内存库并迁移；供测试与一次性工具使用。
    pub fn open_in_memory() -> Result<Self> {
        let db = Self {
            conn: Connection::open_in_memory()?,
            path: None,
        };
        db.configure()?;
        db.migrate()?;
        Ok(db)
    }

    /// 迁移到最新 schema，返回迁移后的 `user_version`。
    ///
    /// 升级已有数据库前先复制一份 `<db>.bak`（ARCH-003 §1）。
    pub fn migrate(&self) -> Result<u32> {
        let current = schema::version(&self.conn)?;
        if current >= schema::SCHEMA_VERSION {
            return Ok(current);
        }
        if current > 0
            && let Some(path) = &self.path
        {
            backup(path)?;
        }
        schema::apply(&self.conn, current)
    }

    /// 当前 `PRAGMA user_version`。
    pub fn schema_version(&self) -> Result<u32> {
        schema::version(&self.conn)
    }

    /// 在标题 / 艺人 / 专辑上做不区分大小写的 `LIKE` 检索（ARCH-003 §5）。
    pub fn search(&self, keyword: &str, limit: usize) -> Result<Vec<Track>> {
        repo::search(&self.conn, keyword, limit)
    }

    /// 最近入库的曲目。
    pub fn list_recent(&self, limit: usize) -> Result<Vec<Track>> {
        repo::list_recent(&self.conn, limit)
    }

    /// 一张专辑的曲目，按 `disc_no, track_no` 排序。
    pub fn tracks_by_album(&self, album_id: i64) -> Result<Vec<Track>> {
        repo::tracks_by_album(&self.conn, album_id)
    }

    /// 一位艺人的曲目（经 `track_artists` 连接）。
    pub fn tracks_by_artist(&self, artist_id: i64) -> Result<Vec<Track>> {
        repo::tracks_by_artist(&self.conn, artist_id)
    }

    /// 登记一个扫描根。
    ///
    /// 路径先转绝对路径；已登记时返回 `Ok(false)`，不覆盖已有时间戳。
    pub fn add_root(&self, root: impl AsRef<Path>) -> Result<bool> {
        let root = std::path::absolute(root.as_ref())?;
        repo::add_root(&self.conn, &root, now_ms())
    }

    /// 全部扫描根（绝对路径，按添加顺序）。
    pub fn list_roots(&self) -> Result<Vec<String>> {
        repo::list_roots(&self.conn)
    }

    /// 曲目数 / 专辑数 / 艺人数 / 总时长 / 最近扫描。
    pub fn stats(&self) -> Result<Stats> {
        repo::stats(&self.conn)
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    fn configure(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }
}

/// 写库事务的辅助器。
///
/// 一个扫描轮次开一个事务，每条记录用 SAVEPOINT 包起来；单条失败只回滚到
/// 该条，不会连累整批。`BATCH_SIZE` 条提交一次，控制 WAL 大小。
pub(crate) struct WriteBatch<'a> {
    conn: &'a Connection,
    pending: usize,
    open: bool,
}

impl<'a> WriteBatch<'a> {
    pub(crate) fn begin(conn: &'a Connection) -> Result<Self> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            conn,
            pending: 0,
            open: true,
        })
    }

    /// 在 SAVEPOINT 里执行一条记录的写入；失败只回滚这一条。
    pub(crate) fn run<T>(&mut self, write: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("SAVEPOINT np_item")?;
        match write(self.conn) {
            Ok(value) => {
                self.conn.execute_batch("RELEASE np_item")?;
                self.pending += 1;
                Ok(value)
            }
            Err(err) => {
                if self
                    .conn
                    .execute_batch("ROLLBACK TO np_item; RELEASE np_item")
                    .is_err()
                {
                    // 事务状态已经不可信，交给 Drop 整体回滚。
                    self.open = false;
                }
                Err(err)
            }
        }
    }

    /// 到 `BATCH_SIZE` 就把当前事务提交并开下一个。
    pub(crate) fn tick(&mut self) -> Result<()> {
        if self.pending >= BATCH_SIZE {
            self.conn.execute_batch("COMMIT; BEGIN IMMEDIATE")?;
            self.pending = 0;
        }
        Ok(())
    }

    /// 提交最后一批。
    pub(crate) fn commit(mut self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.open = false;
        Ok(())
    }
}

impl Drop for WriteBatch<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

/// 当前 Unix 毫秒；系统时钟早于纪元时回退到 0。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 升级前把非空库复制成 `<path>.bak`。
fn backup(path: &Path) -> Result<()> {    let is_non_empty = fs::metadata(path)
        .map(|meta| meta.len() > 0)
        .unwrap_or(false);
    if !is_non_empty {
        return Ok(());
    }
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    fs::copy(path, PathBuf::from(name))?;
    Ok(())
}
