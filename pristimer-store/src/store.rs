//! SQLite 存储层：schema 迁移、会话读写、统计查询。
//!
//! 【本层的核心设计】落库的不是「已经累计了多少」，而是「从哪个时刻开始」
//! —— 这正是第 2 课锚点模型在磁盘上的复刻。因此进程被强杀后，
//! 只要读回 `anchor_at` 就能无损重算出已经过了多久。

use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection};

use crate::session::{
    DailyStat, FinishOutcome, HourTotal, Recovered, Recovery, SessionKind, SessionRow, SessionState,
    TagDayTotal, TagTotal,
};

/// 短于这个时长的会话不落库。
///
/// 现实场景：用户误点「开始」后马上「重置」，会留下大量 0.3 秒的记录，
/// 把统计图彻底污染。这类噪声必须在写入侧挡掉，而不是在展示侧过滤 ——
/// 数据一旦脏了，后面每个查询都要记得带这个条件。
pub const MIN_SESSION_MS: u64 = 10_000;

/// 启动恢复时允许接续的最大跨度。超过它说明应用被关了太久
/// （或系统时间被大幅调整），不应假设用户一直在专注。
pub const MAX_RESUME_GAP_MS: i64 = 12 * 60 * 60 * 1000;

/// 建表语句。`IF NOT EXISTS` 让它可以被安全地重复执行。
///
/// `session` 只此一张事实表，不建预聚合的 `daily_stat`（理由在第 4 课正文）。
/// `setting` 是通用的键值配置表：值为 JSON 文本，读出后由上层反序列化 ——
/// 存储层不关心配置的内部结构，新增配置项不需要改 schema。
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS session (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL,
    tag        TEXT,
    state      TEXT    NOT NULL,
    started_at INTEGER NOT NULL,
    anchor_at  INTEGER NOT NULL,
    ended_at   INTEGER,
    elapsed_ms INTEGER NOT NULL DEFAULT 0,
    limit_ms   INTEGER,
    completed  INTEGER NOT NULL DEFAULT 0,
    note       TEXT
);
CREATE INDEX IF NOT EXISTS idx_session_state   ON session(state);
CREATE INDEX IF NOT EXISTS idx_session_started ON session(started_at);
CREATE TABLE IF NOT EXISTS setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// 存储层错误。
#[derive(Debug)]
pub enum StoreError {
    Db(rusqlite::Error),
    /// 数据库里出现了本程序不会写入的枚举值 —— 说明数据被外部改过。
    UnknownKind(String),
    UnknownState(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(err) => write!(f, "数据库错误: {err}"),
            Self::UnknownKind(raw) => write!(f, "无法识别的会话类型: {raw}"),
            Self::UnknownState(raw) => write!(f, "无法识别的会话状态: {raw}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Db(err) => Some(err),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Db(err)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// 把手写的字符串转成枚举，转不动就报错而不是静默兜底。
fn parse_kind(raw: String) -> Result<SessionKind> {
    SessionKind::parse(&raw).ok_or(StoreError::UnknownKind(raw))
}

fn parse_state(raw: String) -> Result<SessionState> {
    SessionState::parse(&raw).ok_or(StoreError::UnknownState(raw))
}

/// 撞锁时的等待上限。
///
/// 注意 rusqlite 在 `Connection::open` 里**已经**默认设了 5000ms（见
/// `inner_connection.rs` 的 `sqlite3_busy_timeout(db, 5000)`），所以这里写同一
/// 个值不是为了「修好默认值」，而是把这条不变量钉在我们自己这边 —— 换掉底层
/// 驱动、或哪天默认值变了，行为不该跟着变。
///
/// 更要紧的是：**光靠等待解决不了所有锁冲突**。WAL 模式下，一个已经持有读快照
/// 的连接再去写，SQLite 会直接返回 `SQLITE_BUSY_SNAPSHOT`（用户看到的就是
/// `database is locked`），并且**刻意不调用忙等处理器** —— 唯一出路是回滚重来。
/// 这才是「等 5 秒也照样失败」的原因，也是 `open_resilient` 存在的理由。
const BUSY_TIMEOUT_MS: u64 = 5_000;

/// 打开失败后的重试次数与间隔。
///
/// 重试必须**换一条新连接**：过期的是那条连接的读快照，同一连接再试一次
/// 还是同一个结果。文件级争用（另一个实例正在收尾、杀毒软件刚扫过同目录）
/// 也一并被这个循环吃掉。
const OPEN_ATTEMPTS: u32 = 3;
const OPEN_BACKOFF: Duration = Duration::from_millis(400);

/// SQLite 存储句柄。
pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开（或创建）数据库文件。
    ///
    /// 带有限重试：见 `open_resilient`。绝大多数调用方要的就是这个版本 ——
    /// 启动路径上的一次瞬时锁冲突不该让应用起不来。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_resilient(path, OPEN_ATTEMPTS, OPEN_BACKOFF)
    }

    /// 打开失败的分类重试版。
    ///
    /// 只重试「数据库层」的失败（`StoreError::Db`）—— 那是可能自愈的争用
    /// （典型就是 `database is locked`）；数据被外部改坏（`UnknownKind` /
    /// `UnknownState`）重试一万次也一样。
    ///
    /// 每次尝试都新开连接，这一点是必须的：见 `BUSY_TIMEOUT_MS` 的说明 ——
    /// 过期的是「那条连接的读快照」，同一条连接再试一次还是同样的失败。
    pub fn open_resilient(path: impl AsRef<Path>, attempts: u32, backoff: Duration) -> Result<Self> {
        let path = path.as_ref();
        let attempts = attempts.max(1);
        let mut last: Option<StoreError> = None;
        for attempt in 0..attempts {
            // 先把 rusqlite 的错误统一成 StoreError，才能与 `from_conn` 串起来
            let opened = Connection::open(path)
                .map_err(StoreError::from)
                .and_then(Self::from_conn);
            match opened {
                Ok(store) => return Ok(store),
                Err(err) => {
                    if !matches!(err, StoreError::Db(_)) {
                        return Err(err);
                    }
                    last = Some(err);
                    if attempt + 1 < attempts {
                        std::thread::sleep(backoff);
                    }
                }
            }
        }
        Err(last.expect("attempts >= 1 时循环至少执行一次"))
    }

    /// 内存库，供单元测试使用 —— 每个测试拿到一个互不干扰的全新数据库。
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        // 建表要拿写锁，而这是打开路径上唯一会跟别的连接正面碰上的一步 ——
        // 把等待上限钉死（理由见 `BUSY_TIMEOUT_MS`），让「瞬时争用」和
        // 「真的坏了」在错误类型上区分开。
        conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// 建表并设置运行期参数。
    pub fn migrate(&self) -> Result<()> {
        // WAL 让读写不互相阻塞：统计查询（读）不会卡住正在落库的计时线程（写）。
        // 注意这条 pragma 会返回一行结果，所以单独用 pragma_update 执行。
        let _ = self.conn.pragma_update(None, "journal_mode", "WAL");
        self.conn.pragma_update(None, "foreign_keys", "ON")?;
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    // ---------- 写入路径 ----------

    /// 开始一条会话，返回其 id。
    ///
    /// `wall_ms` 用墙钟而不是单调钟 —— 单调钟只在单个进程内有意义，
    /// 而这个值要跨进程存活。
    pub fn begin_session(
        &self,
        kind: SessionKind,
        tag: Option<&str>,
        wall_ms: u64,
        limit_ms: Option<u64>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO session (kind, tag, state, started_at, anchor_at, elapsed_ms, limit_ms, completed)
             VALUES (?1, ?2, 'running', ?3, ?3, 0, ?4, 0)",
            params![
                kind.as_str(),
                tag,
                wall_ms as i64,
                limit_ms.map(|v| v as i64)
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 暂停：把当前段的时长并入 `elapsed_ms`，锚点停在暂停时刻。
    ///
    /// 这一步落库的价值在于：暂停期间崩溃，重算时不会把暂停的那段算进去。
    pub fn pause_session(&self, id: i64, wall_ms: u64, elapsed_ms: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE session SET state = 'paused', elapsed_ms = ?2, anchor_at = ?3
             WHERE id = ?1 AND state = 'running'",
            params![id, elapsed_ms as i64, wall_ms as i64],
        )?;
        Ok(())
    }

    /// 继续：重设锚点，并把「恢复时刻的已用时长」一并写回。
    ///
    /// 为什么必须同时写 `elapsed_ms`：如果只把锚点推到当前时刻，那么下次崩溃时
    /// 重算出的只是「本次恢复之后」的时长，之前累积的那部分会被静默丢掉。
    ///
    /// `state <> 'finished'` 这个条件而不是 `= 'paused'`：恢复出来的记录可能
    /// 停在 `running`（应用被强杀），它同样需要被接续。
    pub fn resume_session(&self, id: i64, wall_ms: u64, elapsed_ms: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE session SET state = 'running', anchor_at = ?2, elapsed_ms = ?3
             WHERE id = ?1 AND state <> 'finished'",
            params![id, wall_ms as i64, elapsed_ms as i64],
        )?;
        Ok(())
    }

    /// 结算会话。
    pub fn finish_session(
        &self,
        id: i64,
        wall_ms: u64,
        elapsed_ms: u64,
        completed: bool,
    ) -> Result<FinishOutcome> {
        if elapsed_ms < MIN_SESSION_MS {
            self.conn
                .execute("DELETE FROM session WHERE id = ?1", params![id])?;
            return Ok(FinishOutcome::Discarded);
        }
        self.conn.execute(
            "UPDATE session SET state = 'finished', ended_at = ?2, elapsed_ms = ?3, completed = ?4
             WHERE id = ?1",
            params![
                id,
                wall_ms as i64,
                elapsed_ms as i64,
                i64::from(completed)
            ],
        )?;
        Ok(FinishOutcome::Saved)
    }

    // ---------- 恢复路径 ----------

    /// 应用启动时调用：找出未结算的会话，结算掉过期的，返回可接续的那条。
    pub fn recover_on_start(&self, now_wall_ms: u64) -> Result<Recovery> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, state, started_at, anchor_at, elapsed_ms, limit_ms
             FROM session
             WHERE state IN ('running', 'paused')
             ORDER BY started_at DESC",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut recovery = Recovery::default();

        for (id, kind_raw, state_raw, started_at, anchor_at, elapsed_ms, limit_ms) in rows {
            let kind = parse_kind(kind_raw)?;
            let state = parse_state(state_raw)?;

            let now = now_wall_ms as i64;

            // 锚点模型重算：运行中的会话把「锚点到现在」补上；暂停的不用。
            // 系统时间被往前调时 saturating_sub 会得到 0，不会出现负时长。
            let raw_elapsed = if state == SessionState::Running {
                elapsed_ms.saturating_add(now.saturating_sub(anchor_at))
            } else {
                elapsed_ms
            };

            // 关闭期间已经越过终点的倒计时：它其实"已经完成了"，不该被接续。
            let reached_limit = matches!(limit_ms, Some(limit) if raw_elapsed >= limit);
            let elapsed_ms = match limit_ms {
                // 封顶，避免把关闭期间多出来的时间记进统计。
                Some(limit) if raw_elapsed >= limit => limit,
                _ => raw_elapsed,
            };

            let expired = now.saturating_sub(started_at) > MAX_RESUME_GAP_MS;
            let age_ms = now_wall_ms.saturating_sub(started_at.max(0) as u64);

            if reached_limit || expired || recovery.resume.is_some() {
                // 已到点 / 已过期 / 已有更新的一条等着接续 —— 这条直接结算。
                self.finish_session(id, now_wall_ms, elapsed_ms as u64, reached_limit)?;
                recovery.settled += 1;
            } else {
                // 这一条交给上层接续。关键动作：把重算出的时长**写回库**，
                // 并转成 `paused`，让后续每次启动都能幂等地读出同一个数值。
                //
                // 少了这一步会出大问题：假设会话 5 分钟时被强杀，用户没管它就把
                // 应用关了，10 小时后再打开 —— 锚点还停在最初的位置，重算出来
                // 就是 10 小时「专注」。写成 paused 之后，缺口不再被累加。
                self.conn.execute(
                    "UPDATE session SET state = 'paused', elapsed_ms = ?2, anchor_at = ?3
                     WHERE id = ?1",
                    params![id, elapsed_ms, now],
                )?;
                recovery.resume = Some(Recovered {
                    id,
                    kind,
                    started_at,
                    elapsed_ms: elapsed_ms as u64,
                    limit_ms: limit_ms.map(|v| v as u64),
                    age_ms,
                });
            }
        }

        Ok(recovery)
    }

    // ---------- 查询路径 ----------

    /// 区间内按本地日期聚合的统计。`from_day` / `to_day` 为 `YYYY-MM-DD`（含两端）。
    pub fn daily_stats(&self, from_day: &str, to_day: &str) -> Result<Vec<DailyStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT day, total_ms, session_count, completed_count FROM (
                 SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS day,
                        SUM(elapsed_ms)  AS total_ms,
                        COUNT(*)         AS session_count,
                        SUM(completed)   AS completed_count
                 FROM session
                 WHERE state = 'finished' AND ended_at IS NOT NULL
                 GROUP BY day
             )
             WHERE day BETWEEN ?1 AND ?2
             ORDER BY day",
        )?;

        let rows = stmt
            .query_map(params![from_day, to_day], |row| {
                Ok(DailyStat {
                    day: row.get(0)?,
                    total_ms: row.get(1)?,
                    session_count: row.get(2)?,
                    completed_count: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    /// 已结算会话的总时长（毫秒）。
    pub fn total_ms(&self) -> Result<i64> {
        let total: Option<i64> = self.conn.query_row(
            "SELECT SUM(elapsed_ms) FROM session WHERE state = 'finished' AND ended_at IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(total.unwrap_or(0))
    }

    /// 全部已结算会话的明细（按开始时间升序）。CSV 导出直接消费。
    pub fn finished_sessions(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, tag, state, started_at, anchor_at, ended_at,
                    elapsed_ms, limit_ms, completed, note
             FROM session
             WHERE state = 'finished' AND ended_at IS NOT NULL
             ORDER BY started_at",
        )?;

        let raw_rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut rows = Vec::with_capacity(raw_rows.len());
        for (
            id,
            kind_raw,
            tag,
            state_raw,
            started_at,
            anchor_at,
            ended_at,
            elapsed_ms,
            limit_ms,
            completed,
            note,
        ) in raw_rows
        {
            rows.push(SessionRow {
                id,
                kind: parse_kind(kind_raw)?,
                tag,
                state: parse_state(state_raw)?,
                started_at,
                anchor_at,
                ended_at,
                elapsed_ms,
                limit_ms,
                completed: completed != 0,
                note,
            });
        }
        Ok(rows)
    }

    /// 已结算会话的数量。
    pub fn finished_count(&self) -> Result<i64> {
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM session WHERE state = 'finished' AND ended_at IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    // ---------- 配置读写 ----------

    /// 读一个配置项。不存在返回 `None`（由上层决定默认值）。
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM setting WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 写一个配置项（upsert）。同 key 覆盖旧值。
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 按标签聚合的已结算时长（降序）。无标签的会话 tag 为空字符串，
    /// 由展示层起名（如「未标注」）。
    ///
    /// `from_day` / `to_day` 为本地日期 `YYYY-MM-DD`（含两端），按会话
    /// **开始时刻**的本地日过滤 —— 与 `daily_stats` 同一口径。
    pub fn tag_totals_between(&self, from_day: &str, to_day: &str) -> Result<Vec<TagTotal>> {
        let mut stmt = self.conn.prepare(
            "SELECT IFNULL(tag, '') AS tag,
                    SUM(elapsed_ms) AS total_ms,
                    COUNT(*)        AS session_count
             FROM session
             WHERE state = 'finished' AND ended_at IS NOT NULL
               AND date(started_at / 1000, 'unixepoch', 'localtime') BETWEEN ?1 AND ?2
             GROUP BY tag
             ORDER BY total_ms DESC",
        )?;

        let rows = stmt
            .query_map(params![from_day, to_day], |row| {
                Ok(TagTotal {
                    tag: row.get(0)?,
                    total_ms: row.get(1)?,
                    session_count: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    /// 重命名（或合并、或清除）一个标签：把所有 `tag = from` 的会话改为
    /// `to`。`from` 传空串匹配「未标注」（库中为 NULL）；`to` 传 `None`
    /// 表示清空为未标注。返回改写的行数。
    ///
    /// 「合并」就是「重命名到一个已存在的名字」——同一句 UPDATE 天然幂等，
    /// 不需要单独的合并逻辑。
    pub fn rename_tag(&self, from: &str, to: Option<&str>) -> Result<usize> {
        let changed = self.conn.execute(
            "UPDATE session SET tag = ?2 WHERE IFNULL(tag, '') = ?1",
            params![from, to],
        )?;
        Ok(changed)
    }

    /// 区间内按「本地日 × 标签」聚合的专注时长。目标达成天数、
    /// 周报这类「哪天哪个科目学了多久」的需求直接消费。
    ///
    /// `from_day` / `to_day` 为本地日期 `YYYY-MM-DD`（含两端），口径同
    /// `daily_stats` / `tag_totals_between`。
    pub fn tag_daily_totals_between(
        &self,
        from_day: &str,
        to_day: &str,
    ) -> Result<Vec<TagDayTotal>> {
        let mut stmt = self.conn.prepare(
            "SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS day,
                    IFNULL(tag, '')            AS tag,
                    SUM(elapsed_ms)            AS total_ms
             FROM session
             WHERE state = 'finished' AND ended_at IS NOT NULL
               AND date(started_at / 1000, 'unixepoch', 'localtime') BETWEEN ?1 AND ?2
             GROUP BY day, tag
             ORDER BY day, total_ms DESC",
        )?;

        let rows = stmt
            .query_map(params![from_day, to_day], |row| {
                Ok(TagDayTotal {
                    day: row.get(0)?,
                    tag: row.get(1)?,
                    total_ms: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    /// 区间内按「会话开始时刻的本地小时」（0–23）聚合的专注时长。
    ///
    /// 口径说明：跨小时的会话（如 13:50–14:40）整体归入**开始小时**，
    /// 不按分钟拆分 —— 拆分要引入区间求交，复杂度翻倍而洞察增量有限；
    /// 「通常在哪个钟点坐下开始学」正是本查询要回答的问题。
    /// 没有记录的小时不会出现在结果里，由展示层补零。
    ///
    /// `from_day` / `to_day` 为本地日期 `YYYY-MM-DD`（含两端），口径同
    /// `daily_stats` / `tag_totals_between`。
    pub fn hour_totals_between(&self, from_day: &str, to_day: &str) -> Result<Vec<HourTotal>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(strftime('%H', started_at / 1000, 'unixepoch', 'localtime') AS INTEGER)
                    AS hour,
                    SUM(elapsed_ms) AS total_ms,
                    COUNT(*)        AS session_count
             FROM session
             WHERE state = 'finished' AND ended_at IS NOT NULL
               AND date(started_at / 1000, 'unixepoch', 'localtime') BETWEEN ?1 AND ?2
             GROUP BY hour
             ORDER BY hour",
        )?;

        let rows = stmt
            .query_map(params![from_day, to_day], |row| {
                Ok(HourTotal {
                    hour: row.get::<_, i64>(0)? as u32,
                    total_ms: row.get(1)?,
                    session_count: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    /// 直接暴露底层连接，供上层做临时查询。
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}
