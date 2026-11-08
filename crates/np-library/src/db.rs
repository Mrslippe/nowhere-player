//! 连接管理与迁移。
//!
//! 一个 [`Database`] 持有一条连接；扫描器的 writer 在调用线程上复用这条连接，
//! 因此任意时刻只有一个线程写库（见 `assets` 之外的 ARCH-003 §0）。

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::Result;
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

/// 升级前把非空库复制成 `<path>.bak`。
fn backup(path: &Path) -> Result<()> {
    let is_non_empty = fs::metadata(path).map(|meta| meta.len() > 0).unwrap_or(false);
    if !is_non_empty {
        return Ok(());
    }
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    fs::copy(path, PathBuf::from(name))?;
    Ok(())
}
