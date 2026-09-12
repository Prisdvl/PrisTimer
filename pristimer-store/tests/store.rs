//! 存储层的集成测试。
//!
//! 放在 `tests/` 目录而不是源码内联：这里只用公开 API，正好检验
//! 「这个 crate 的对外接口是否真的够用」。

use pristimer_store::{FinishOutcome, SessionKind, Store, MAX_RESUME_GAP_MS};

const T0: u64 = 1_700_000_000_000; // 2023-11-14 22:13:20 UTC
const SEC: u64 = 1_000;
const MIN: u64 = 60 * SEC;
const HOUR: u64 = 60 * MIN;

fn store() -> Store {
    Store::open_in_memory().expect("内存库应当总能打开")
}

#[test]
fn finished_session_lands_in_stats() {
    let store = store();
    let id = store
        .begin_session(SessionKind::Stopwatch, Some("数学"), T0, None)
        .unwrap();

    // 模拟：跑 5 分钟 → 暂停 → 隔 3 分钟继续 → 又跑 5 分钟 → 手动结束
    store.pause_session(id, T0 + 5 * MIN, 5 * MIN).unwrap();
    store.resume_session(id, T0 + 8 * MIN, 5 * MIN).unwrap();
    let outcome = store
        .finish_session(id, T0 + 13 * MIN, 10 * MIN, false)
        .unwrap();

    assert_eq!(outcome, FinishOutcome::Saved);
    assert_eq!(store.total_ms().unwrap(), (10 * MIN) as i64);
    assert_eq!(store.finished_count().unwrap(), 1);
}

#[test]
fn short_session_is_discarded() {
    let store = store();
    let id = store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();

    // 误触「开始」后 3 秒就重置 —— 不该留下记录污染统计
    let outcome = store
        .finish_session(id, T0 + 3 * SEC, 3 * SEC, false)
        .unwrap();

    assert_eq!(outcome, FinishOutcome::Discarded);
    assert_eq!(store.finished_count().unwrap(), 0);
    assert_eq!(store.total_ms().unwrap(), 0);
}

#[test]
fn running_session_is_resumed_with_recomputed_elapsed() {
    let store = store();
    store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();

    // 应用在跑了 5 分钟后被杀，重新启动
    let recovery = store.recover_on_start(T0 + 5 * MIN).unwrap();

    let resumed = recovery.resume.expect("应当有一条可接续的会话");
    assert_eq!(resumed.elapsed_ms, 5 * MIN);
    assert_eq!(resumed.age_ms, 5 * MIN);
    assert_eq!(recovery.settled, 0);
}

#[test]
fn paused_session_resumes_without_counting_the_gap() {
    let store = store();
    let id = store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();
    store.pause_session(id, T0 + 5 * MIN, 5 * MIN).unwrap();

    // 暂停后又过了 40 分钟才重新打开应用：这 40 分钟不是专注时间
    let recovery = store.recover_on_start(T0 + 45 * MIN).unwrap();

    assert_eq!(recovery.resume.unwrap().elapsed_ms, 5 * MIN);
}

#[test]
fn countdown_reaching_limit_while_closed_is_settled() {
    let store = store();
    store
        .begin_session(SessionKind::Countdown, None, T0, Some(25 * MIN))
        .unwrap();

    // 关掉应用两小时 —— 倒计时早已到点
    let recovery = store.recover_on_start(T0 + 2 * HOUR).unwrap();

    assert!(recovery.resume.is_none(), "已到点的倒计时不该被接续");
    assert_eq!(recovery.settled, 1);
    // 封顶到 25 分钟，而不是把两小时全记进去
    assert_eq!(store.total_ms().unwrap(), (25 * MIN) as i64);
}

#[test]
fn stale_session_is_settled_instead_of_resumed() {
    let store = store();
    store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();

    let beyond = T0 + MAX_RESUME_GAP_MS as u64 + MIN;
    let recovery = store.recover_on_start(beyond).unwrap();

    assert!(recovery.resume.is_none(), "超过接续上限的会话应当被结算");
    assert_eq!(recovery.settled, 1);
    assert_eq!(store.total_ms().unwrap(), (MAX_RESUME_GAP_MS as u64 + MIN) as i64);
}

#[test]
fn only_the_newest_unfinished_session_is_resumed() {
    let store = store();
    store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();
    store
        .begin_session(SessionKind::Stopwatch, None, T0 + 10 * MIN, None)
        .unwrap();

    let recovery = store.recover_on_start(T0 + 20 * MIN).unwrap();

    let resumed = recovery.resume.unwrap();
    assert_eq!(resumed.started_at, (T0 + 10 * MIN) as i64);
    assert_eq!(resumed.elapsed_ms, 10 * MIN);
    assert_eq!(recovery.settled, 1);
}

#[test]
fn recovery_is_idempotent_across_restarts() {
    let store = store();
    store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();

    // 第一次启动：会话已经跑了 5 分钟，被恢复出来
    let first = store.recover_on_start(T0 + 5 * MIN).unwrap();
    assert_eq!(first.resume.unwrap().elapsed_ms, 5 * MIN);

    // 用户没管它就直接关掉应用；两小时后再打开。
    // 若恢复时不把时长写回库，这里会算出 2 小时「专注」。
    let second = store.recover_on_start(T0 + 2 * HOUR + 5 * MIN).unwrap();
    assert_eq!(
        second.resume.unwrap().elapsed_ms,
        5 * MIN,
        "应用关闭期间的时间不该被累加进专注时长"
    );
}

#[test]
fn resume_records_elapsed_so_a_later_crash_does_not_lose_it() {
    let store = store();
    store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();

    // 恢复出 5 分钟，用户按下「继续」
    let recovered = store.recover_on_start(T0 + 5 * MIN).unwrap().resume.unwrap();
    store
        .resume_session(recovered.id, T0 + 5 * MIN, recovered.elapsed_ms)
        .unwrap();

    // 又跑了 10 分钟，应用再次被杀
    let again = store
        .recover_on_start(T0 + 15 * MIN)
        .unwrap()
        .resume
        .unwrap();

    // 5 + 10 = 15，而不是只剩第二次的 10 分钟
    assert_eq!(again.elapsed_ms, 15 * MIN);
    assert_eq!(again.id, recovered.id, "接续的必须是同一条记录");
}

#[test]
fn unfinished_sessions_are_excluded_from_stats() {
    let store = store();
    let id = store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();
    store.pause_session(id, T0 + 5 * MIN, 5 * MIN).unwrap();

    // 尚未结算的会话不进统计 —— 否则「今日专注」会随着秒表跳动
    assert_eq!(store.total_ms().unwrap(), 0);
    assert_eq!(store.finished_count().unwrap(), 0);
}

#[test]
fn daily_stats_group_by_local_day() {
    let store = store();

    // 同一自然日的两条
    let a = store
        .begin_session(SessionKind::Stopwatch, None, T0, None)
        .unwrap();
    store.finish_session(a, T0 + MIN, 5 * MIN, false).unwrap();
    let b = store
        .begin_session(SessionKind::Stopwatch, None, T0 + 60 * SEC, None)
        .unwrap();
    store.finish_session(b, T0 + MIN, 7 * MIN, true).unwrap();

    // 30 小时之后：无论本地时区如何，必然落在另一个自然日
    // （一个自然日最多 25 小时，30 > 25）
    let t2 = T0 + 30 * HOUR;
    let c = store
        .begin_session(SessionKind::Countdown, None, t2, Some(25 * MIN))
        .unwrap();
    store.finish_session(c, t2 + 25 * MIN, 25 * MIN, true).unwrap();

    let stats = store.daily_stats("1900-01-01", "2999-12-31").unwrap();
    assert_eq!(stats.len(), 2, "应当聚合成两天");

    let first = &stats[0];
    assert_eq!(first.session_count, 2);
    assert_eq!(first.total_ms, (12 * MIN) as i64);
    assert_eq!(first.completed_count, 1);

    let second = &stats[1];
    assert_eq!(second.session_count, 1);
    assert_eq!(second.total_ms, (25 * MIN) as i64);
    assert_eq!(second.completed_count, 1);

    assert_ne!(first.day, second.day);
    assert!(first.day < second.day, "日期字符串应当按字典序递增");
}

#[test]
fn daily_stats_respects_the_range() {
    let store = store();
    let t2 = T0 + 30 * HOUR;
    for started in [T0, t2] {
        let id = store
            .begin_session(SessionKind::Stopwatch, None, started, None)
            .unwrap();
        store.finish_session(id, started + 5 * MIN, 5 * MIN, false).unwrap();
    }

    let all = store.daily_stats("1900-01-01", "2999-12-31").unwrap();
    assert_eq!(all.len(), 2);

    // 只查第一天，应当只拿到一条
    let only_first = store.daily_stats("1900-01-01", &all[0].day).unwrap();
    assert_eq!(only_first.len(), 1);
    assert_eq!(only_first[0].day, all[0].day);
}

#[test]
fn sessions_survive_reopening_the_file() {
    let dir = std::env::temp_dir().join("pristimer_store_reopen_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pristimer.db");

    let total = {
        let store = Store::open(&path).unwrap();
        let id = store
            .begin_session(SessionKind::Stopwatch, Some("英语"), T0, None)
            .unwrap();
        store
            .finish_session(id, T0 + 25 * MIN, 25 * MIN, false)
            .unwrap();
        store.total_ms().unwrap()
    }; // 连接在此处关闭

    assert_eq!(total, (25 * MIN) as i64);

    // 重新打开：数据必须还在，且 schema 迁移可以重复执行
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.total_ms().unwrap(), (25 * MIN) as i64);
    assert_eq!(reopened.finished_count().unwrap(), 1);
    drop(reopened);

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "serde")]
#[test]
fn daily_stat_serializes_to_camel_case() {
    use pristimer_store::DailyStat;

    let stat = DailyStat {
        day: "2026-09-10".to_owned(),
        total_ms: 5_400_000,
        session_count: 3,
        completed_count: 2,
    };

    let json = serde_json::to_string(&stat).unwrap();
    assert_eq!(
        json,
        r#"{"day":"2026-09-10","totalMs":5400000,"sessionCount":3,"completedCount":2}"#
    );
}

#[test]
fn settings_round_trip_and_overwrite() {
    let store = Store::open_in_memory().unwrap();

    // 不存在的键返回 None，而不是报错
    assert_eq!(store.get_setting("pomodoro").unwrap(), None);

    store
        .set_setting("pomodoro", r#"{"focus_ms":1500000}"#)
        .unwrap();
    assert_eq!(
        store.get_setting("pomodoro").unwrap().as_deref(),
        Some(r#"{"focus_ms":1500000}"#)
    );

    // 同 key 再写 = 覆盖
    store.set_setting("pomodoro", "{}").unwrap();
    assert_eq!(store.get_setting("pomodoro").unwrap().as_deref(), Some("{}"));
}

#[test]
fn settings_survive_reopening_and_are_isolated_per_key() {
    let dir = std::env::temp_dir().join("pristimer_store_settings_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.db");

    {
        let store = Store::open(&path).unwrap();
        store.set_setting("a", "1").unwrap();
        store.set_setting("b", "2").unwrap();
    }

    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.get_setting("a").unwrap().as_deref(), Some("1"));
    assert_eq!(reopened.get_setting("b").unwrap().as_deref(), Some("2"));
    assert_eq!(reopened.get_setting("c").unwrap(), None);
    drop(reopened);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn finished_sessions_returns_rows_in_start_order() {
    use pristimer_store::{SessionKind, SessionState};

    let store = Store::open_in_memory().unwrap();
    // 故意乱序写入：第二条开始得更晚但先结算
    let late = store
        .begin_session(SessionKind::Stopwatch, Some("英语"), 3 * MIN, None)
        .unwrap();
    let early = store
        .begin_session(SessionKind::Countdown, None, MIN, Some(25 * MIN))
        .unwrap();
    store
        .finish_session(early, 2 * MIN, 25 * MIN, true)
        .unwrap();
    store
        .finish_session(late, 4 * MIN, MIN, false)
        .unwrap();

    // 未结算的会话不出现在明细里
    let open = store
        .begin_session(SessionKind::Stopwatch, None, 5 * MIN, None)
        .unwrap();
    store.pause_session(open, 5 * MIN + 3_000, 3_000).unwrap();

    let rows = store.finished_sessions().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, early);
    assert_eq!(rows[0].kind, SessionKind::Countdown);
    assert_eq!(rows[0].state, SessionState::Finished);
    assert_eq!(rows[0].started_at, MIN as i64);
    assert_eq!(rows[0].elapsed_ms, (25 * MIN) as i64);
    assert!(rows[0].completed);
    assert_eq!(rows[1].id, late);
    assert_eq!(rows[1].kind, SessionKind::Stopwatch);
    assert_eq!(rows[1].tag.as_deref(), Some("英语"));
    assert!(!rows[1].completed);
}

#[test]
fn tag_totals_group_and_rank_by_duration() {
    use pristimer_store::SessionKind;

    let store = Store::open_in_memory().unwrap();
    let tag = |name: &str| {
        store
            .begin_session(SessionKind::Countdown, Some(name), MIN, None)
            .unwrap()
    };
    let untagged = store
        .begin_session(SessionKind::Stopwatch, None, MIN, None)
        .unwrap();

    let math = tag("高数");
    let english = tag("英语");
    store.finish_session(math, 2 * MIN, 25 * MIN, true).unwrap();
    store
        .finish_session(english, 3 * MIN, 10 * MIN, false)
        .unwrap();
    store
        .finish_session(untagged, 4 * MIN, 5 * MIN, false)
        .unwrap();
    // 第二个高数会话并入同一行
    let math2 = tag("高数");
    store.finish_session(math2, 6 * MIN, 15 * MIN, true).unwrap();

    // 短于 MIN_SESSION_MS 的会话被丢弃，不进统计
    let noise = tag("线代");
    store.finish_session(noise, 7 * MIN, 3_000, false).unwrap();

    // 宽范围覆盖全部（1970 年的 epoch 毫秒本地日，负偏移时区也不越界）
    let totals = store
        .tag_totals_between("1969-12-31", "1971-12-31")
        .unwrap();
    assert_eq!(totals.len(), 3, "丢弃的 3 秒会话不该出现");
    assert_eq!(totals[0].tag, "高数");
    assert_eq!(totals[0].total_ms, (40 * MIN) as i64);
    assert_eq!(totals[0].session_count, 2);
    assert_eq!(totals[1].tag, "英语");
    assert_eq!(totals[2].tag, "", "无标签归入空字符串");
    assert_eq!(totals[2].total_ms, (5 * MIN) as i64);
}

#[test]
fn tag_totals_respects_the_range() {
    use pristimer_store::SessionKind;

    let store = Store::open_in_memory().unwrap();
    // 早会话：epoch 后 1 分钟（1970-01-01，任何时区都在宽范围里）
    let early = store
        .begin_session(SessionKind::Countdown, Some("高数"), MIN, None)
        .unwrap();
    store
        .finish_session(early, 2 * MIN, 20 * MIN, true)
        .unwrap();
    // 晚会话：epoch 后 30 天（1970-01-31 或 01-30，取决于时区偏移方向）
    let late = store
        .begin_session(
            SessionKind::Countdown,
            Some("英语"),
            30 * 24 * 60 * MIN,
            None,
        )
        .unwrap();
    store
        .finish_session(late, 30 * 24 * 60 * MIN + MIN, 10 * MIN, true)
        .unwrap();

    // 只取第一天的范围：只有早会话命中
    let day1 = store
        .tag_totals_between("1969-12-30", "1970-01-02")
        .unwrap();
    assert_eq!(day1.len(), 1);
    assert_eq!(day1[0].tag, "高数");

    // 只取月末的范围：只有晚会话命中
    let month_end = store
        .tag_totals_between("1970-01-20", "1970-02-05")
        .unwrap();
    assert_eq!(month_end.len(), 1);
    assert_eq!(month_end[0].tag, "英语");

    // 空范围：谁都不命中
    assert!(
        store
            .tag_totals_between("2000-01-01", "2000-01-02")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn rename_tag_rewrites_and_counts() {
    use pristimer_store::SessionKind;

    let store = Store::open_in_memory().unwrap();
    let s1 = store
        .begin_session(SessionKind::Stopwatch, Some("高数a"), MIN, None)
        .unwrap();
    store
        .finish_session(s1, 2 * MIN, 15 * MIN, true)
        .unwrap();
    let s2 = store
        .begin_session(SessionKind::Stopwatch, Some("高数a"), 3 * MIN, None)
        .unwrap();
    store.finish_session(s2, 4 * MIN, 5 * MIN, false).unwrap();
    let other = store
        .begin_session(SessionKind::Stopwatch, Some("英语"), 5 * MIN, None)
        .unwrap();
    store.finish_session(other, 6 * MIN, 8 * MIN, true).unwrap();

    // 重命名「高数a」→「高数」：改写 2 行，其他标签不动
    let changed = store.rename_tag("高数a", Some("高数")).unwrap();
    assert_eq!(changed, 2);
    let totals = store.tag_totals_between("1970-01-01", "2999-12-31").unwrap();
    assert_eq!(totals.len(), 2);
    let math = totals.iter().find(|t| t.tag == "高数").unwrap();
    assert_eq!(math.session_count, 2);
    assert!(totals.iter().all(|t| t.tag != "高数a"));
    assert!(totals.iter().any(|t| t.tag == "英语"));

    // 再改一次：没有可改的行，返回 0（幂等）
    assert_eq!(store.rename_tag("高数a", Some("高数")).unwrap(), 0);
}

#[test]
fn rename_tag_merges_into_existing() {
    use pristimer_store::SessionKind;

    let store = Store::open_in_memory().unwrap();
    let a = store
        .begin_session(SessionKind::Stopwatch, Some("线代a"), MIN, None)
        .unwrap();
    store.finish_session(a, 2 * MIN, 12 * MIN, true).unwrap();
    let b = store
        .begin_session(SessionKind::Stopwatch, Some("线代"), 3 * MIN, None)
        .unwrap();
    store.finish_session(b, 4 * MIN, 30 * MIN, true).unwrap();

    // 合并：与重命名是同一个操作，聚合后只剩一行
    let changed = store.rename_tag("线代a", Some("线代")).unwrap();
    assert_eq!(changed, 1);
    let totals = store.tag_totals_between("1970-01-01", "2999-12-31").unwrap();
    assert_eq!(totals.len(), 1);
    assert_eq!(totals[0].tag, "线代");
    assert_eq!(totals[0].total_ms, (12 + 30) * 60 * 1000);
    assert_eq!(totals[0].session_count, 2);
}

#[test]
fn rename_tag_untouched_and_clear() {
    use pristimer_store::SessionKind;

    let store = Store::open_in_memory().unwrap();
    // 未标注：库中 tag 为 NULL
    let untagged = store
        .begin_session(SessionKind::Stopwatch, None, MIN, None)
        .unwrap();
    store
        .finish_session(untagged, 2 * MIN, 9 * MIN, true)
        .unwrap();
    let math = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), 3 * MIN, None)
        .unwrap();
    store.finish_session(math, 4 * MIN, 20 * MIN, true).unwrap();

    // 把「未标注」归类到「英语」：空串匹配 NULL
    let changed = store.rename_tag("", Some("英语")).unwrap();
    assert_eq!(changed, 1);
    let totals = store.tag_totals_between("1970-01-01", "2999-12-31").unwrap();
    assert_eq!(totals.len(), 2);
    assert!(totals.iter().any(|t| t.tag == "英语"));
    assert!(!totals.iter().any(|t| t.tag == ""));

    // 反向操作：把「英语」清空回未标注（to = None）
    let cleared = store.rename_tag("英语", None).unwrap();
    assert_eq!(cleared, 1);
    let totals = store.tag_totals_between("1970-01-01", "2999-12-31").unwrap();
    let blank = totals.iter().find(|t| t.tag == "").unwrap();
    assert_eq!(blank.session_count, 1);
}

#[test]
fn tag_daily_totals_group_by_day_and_tag() {
    use pristimer_store::SessionKind;

    let store = Store::open_in_memory().unwrap();
    let start = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), MIN, None)
        .unwrap();
    store.finish_session(start, 2 * MIN, 30 * MIN, true).unwrap();
    let second = store
        .begin_session(SessionKind::Stopwatch, Some("英语"), MIN, None)
        .unwrap();
    store
        .finish_session(second, 2 * MIN + SEC, 10 * MIN, true)
        .unwrap();

    // 次日（epoch + 1 天）再来一局高数
    let next_day = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), 24 * 60 * MIN, None)
        .unwrap();
    store
        .finish_session(next_day, 24 * 60 * MIN + MIN, 45 * MIN, true)
        .unwrap();

    // 范围外的老会话（epoch + 10 天，落在 1970-01-11，必然被排除）
    let ancient = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), 10 * 24 * 60 * MIN, None)
        .unwrap();
    store
        .finish_session(ancient, 10 * 24 * 60 * MIN + SEC, 99 * MIN, true)
        .unwrap();

    let rows = store
        .tag_daily_totals_between("1970-01-01", "1970-01-02")
        .unwrap();
    assert_eq!(rows.len(), 3);

    // 第一天两行：高数在前（30 分钟 > 10 分钟）
    assert_eq!(rows[0].tag, "高数");
    assert_eq!(rows[0].total_ms, 30 * 60 * 1000);
    assert_eq!(rows[1].tag, "英语");
    assert_eq!(rows[1].total_ms, 10 * 60 * 1000);
    // 第二天一行
    assert_eq!(rows[2].tag, "高数");
    assert_eq!(rows[2].total_ms, 45 * 60 * 1000);
    assert!(rows[2].day != rows[0].day, "第二行的日期必须不同于第一天");

    // 空范围
    assert!(store
        .tag_daily_totals_between("2000-01-01", "2000-01-02")
        .unwrap()
        .is_empty());
}

#[test]
fn hour_totals_group_by_start_hour() {
    let store = store();

    // 基准时刻取 epoch + 12 小时（UTC 正午）：离本地午夜足够远，
    // 任何时区下 t0 与 t0+3h 都落在同一个本地日内，不会出现跨日歧义。
    let base: u64 = 12 * HOUR;
    // 同一小时的两条会话 —— 验证同桶合并
    let a = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), base, None)
        .unwrap();
    store
        .finish_session(a, base + MIN, 25 * MIN, true)
        .unwrap();
    let b = store
        .begin_session(SessionKind::Stopwatch, Some("英语"), base, None)
        .unwrap();
    store
        .finish_session(b, base + 2 * MIN, 10 * MIN, false)
        .unwrap();
    // 三小时后的另一局 —— 验证按开始小时分桶
    let later = base + 3 * HOUR;
    let c = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), later, None)
        .unwrap();
    store
        .finish_session(c, later + MIN, 30 * MIN, true)
        .unwrap();
    // 范围外：epoch + 10 天，必然被排除
    let ancient = 10 * 24 * HOUR;
    let d = store
        .begin_session(SessionKind::Stopwatch, Some("高数"), ancient, None)
        .unwrap();
    store
        .finish_session(d, ancient + SEC, 99 * MIN, true)
        .unwrap();

    // 期望小时从同一存储引擎现查（strftime 的时区转换由 SQLite 保证，
    // 这里验证的是聚合与归属逻辑本身，不重复断言时区行为）。
    let local_hour = |ms: u64| -> u32 {
        store
            .conn()
            .query_row(
                "SELECT CAST(strftime('%H', ?1 / 1000, 'unixepoch', 'localtime') AS INTEGER)",
                [ms as i64],
                |row| row.get::<_, i64>(0),
            )
            .unwrap() as u32
    };
    let h0 = local_hour(base);
    let h3 = local_hour(later);
    assert_eq!(h3, (h0 + 3) % 24, "相隔三小时的会话应落在相差三小时的桶");

    let rows = store
        .hour_totals_between("1970-01-01", "1970-01-02")
        .unwrap();
    assert_eq!(rows.len(), 2, "只应出现两个小时的桶（范围外的被排除）");

    let by_hour: std::collections::HashMap<u32, (i64, i64)> = rows
        .iter()
        .map(|r| (r.hour, (r.total_ms, r.session_count)))
        .collect();
    // 同一小时的两条会话合并：25 + 10 = 35 分钟、2 次
    assert_eq!(
        by_hour.get(&h0),
        Some(&(35 * 60 * 1000, 2)),
        "同一小时的多条会话必须合并进同一个桶"
    );
    assert_eq!(by_hour.get(&h3), Some(&(30 * 60 * 1000, 1)));

    // 空范围
    assert!(store
        .hour_totals_between("2000-01-01", "2000-01-02")
        .unwrap()
        .is_empty());
}

/// 回归：**两个连接同时打开同一个库文件**。
///
/// 这是「偶发闪退」的现场：应用启动要 4–6 秒，用户等不及再点一次图标，
/// 第二个实例起来后 `Store::open` 撞上第一个实例持有的写锁。当时
/// `busy_timeout` 还是默认的 0，SQLite 立刻返回 `SQLITE_BUSY`，
/// 上层看到的就是 `database is locked` —— setup 钩子失败 → 进程凭空消失。
///
/// 这条测试锁住两件事：
///   ① 两个连接可以共存（WAL 模式生效，读写不互斥）；
///   ② 后开的那个也能写入（`busy_timeout` 让它等锁而不是当场失败）。
#[test]
fn two_connections_can_share_one_file() {
    let dir = std::env::temp_dir().join("pristimer_store_concurrent_open_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pristimer.db");

    // 先建库（第一个「实例」）
    let first = Store::open(&path).unwrap();
    let first_id = first
        .begin_session(SessionKind::Stopwatch, Some("英语"), T0, None)
        .unwrap();

    // 第二个「实例」在第一个还活着的时候打开 —— 过去这里会报 database is locked
    let second = Store::open(&path).unwrap();
    let second_id = second
        .begin_session(SessionKind::Countdown, Some("数学"), T0 + SEC, Some(25 * MIN))
        .unwrap();

    // 两边都能写：第一个改自己那条，第二个改自己那条
    first.finish_session(first_id, T0 + 25 * MIN, 25 * MIN, false).unwrap();
    second
        .finish_session(second_id, T0 + 30 * MIN, 29 * MIN, false)
        .unwrap();

    // 从第三个连接读，两条都在（说明前两个写入都真正落盘了）
    drop(first);
    drop(second);
    let reader = Store::open(&path).unwrap();
    let rows = reader.finished_sessions().unwrap();
    assert_eq!(rows.len(), 2, "两个连接写入的会话都应落盘");
    let tags: Vec<String> = rows.iter().filter_map(|r| r.tag.clone()).collect();
    assert!(tags.contains(&"英语".to_string()), "第一个连接的写入丢失");
    assert!(tags.contains(&"数学".to_string()), "第二个连接的写入丢失");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 回归：`open` 在**目录级争用**下应当自愈，而不是把失败直接抛给上层。
///
/// 产品里用的是 `Store::open`（3 次尝试 + 400ms 退避）；这里把重试次数与
/// 退避都调小，只为让测试跑得快，验证的是同一条代码路径：
/// 反复打开同一个库文件必须次次成功，且每次拿到的库都能正常读写。
#[test]
fn repeated_open_on_same_file_always_succeeds() {
    let dir = std::env::temp_dir().join("pristimer_store_retry_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pristimer.db");

    // 第一次：建库 + 写一条
    let store = Store::open_resilient(&path, 1, std::time::Duration::from_millis(10)).unwrap();
    let id = store
        .begin_session(SessionKind::Stopwatch, Some("英语"), T0, None)
        .unwrap();
    store
        .finish_session(id, T0 + 25 * MIN, 25 * MIN, false)
        .unwrap();
    drop(store);

    // 之后反复重开（模拟反复启动 / 反复迁移 schema）：不能有任何一次失败
    for round in 0..3 {
        let again = Store::open_resilient(&path, 3, std::time::Duration::from_millis(10))
            .unwrap_or_else(|err| panic!("第 {round} 次重开失败: {err}"));
        assert_eq!(again.finished_count().unwrap(), 1, "第 {round} 次重开后数据应完好");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// 回归（带负对照）：**「database is locked」是等不掉的，只能换连接重来**。
///
/// 这条测试是从一次真实的启动失败反推出来的，它要钉住的正是那个反直觉的点：
/// rusqlite 在 `Connection::open` 里**已经**把 `busy_timeout` 设成 5 秒了
/// （`inner_connection.rs` 的 `sqlite3_busy_timeout(db, 5000)`）。所以
/// `database is locked` 并不是「没设等待」造成的 —— 有一类锁冲突
/// SQLite **刻意不调用忙等处理器**：
///
///   WAL 模式下，一个已经持有读快照的连接再去写，会直接拿到
///   `SQLITE_BUSY_SNAPSHOT`。SQLite 不去等，因为等也等不来 ——
///   它的快照已经过期了，唯一出路是回滚、换一条新连接重来。
///
/// 这正好是启动路径的形状：`migrate()` 先读一次 pragma、再写 schema，
/// 中间只要**另一个实例**提交了一笔（应用启动要 4–6 秒，用户等不及再点一次
/// 图标极常见），后起的那个就会拿到 `database is locked` → setup 钩子返回 Err
/// → Tauri panic → 进程凭空消失。
///
/// 三段断言依次是：
///   ① 读快照过期的连接去写 → **立刻**失败且错误含 `locked`（负对照：等待无效）
///   ② 新建连接（= `open_resilient` 下一次尝试做的事）→ 立刻写成功
///   ③ 产品入口 `Store::open` 在同一现场下不会把失败抛出去
#[test]
fn a_stale_read_snapshot_locks_immediately_but_a_fresh_connection_recovers() {
    use std::time::{Duration, Instant};

    let dir = std::env::temp_dir().join("pristimer_store_stale_snapshot_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pristimer.db");

    // 建库（顺带把 journal_mode 切到 WAL —— 这个机制只在 WAL 下成立）
    {
        let store = Store::open(&path).unwrap();
        store.set_setting("warmup", "1").unwrap();
    }

    // 「另一个实例」：一直活着并在写
    let other = Store::open(&path).unwrap();

    // 「后起的那个实例」：先读一次，把读快照钉住（migrate 里读 pragma 就是这一步）
    let stale = rusqlite::Connection::open(&path).unwrap();
    stale.execute_batch("BEGIN").unwrap();
    let _: i64 = stale
        .query_row("SELECT COUNT(*) FROM session", [], |row| row.get(0))
        .unwrap();

    // 另一个实例提交一笔 —— 此刻 stale 手里的快照已经作废
    other.set_setting("concurrent", "1").unwrap();

    // ① 负对照：再去写，应当「立刻」失败，而不是等满 5 秒
    let t0 = Instant::now();
    let err = stale
        .execute(
            "INSERT OR REPLACE INTO setting(key, value) VALUES ('stale', '1')",
            [],
        )
        .expect_err("读快照已作废，这次写入必须失败");
    let waited = t0.elapsed();
    assert!(
        err.to_string().contains("locked"),
        "期望错误信息含 locked（用户看到的那句），实际是 {err}"
    );
    assert!(
        waited < Duration::from_millis(500),
        "它竟然等了 {waited:?} —— 说明命中的不是「忙等救不了」的那类冲突，\
         这条测试就没有在测我们以为的东西"
    );
    drop(stale);

    // ② 换一条新连接：拿到的是新快照，立刻就能写
    let fresh = rusqlite::Connection::open(&path).unwrap();
    fresh
        .execute(
            "INSERT OR REPLACE INTO setting(key, value) VALUES ('fresh', '1')",
            [],
        )
        .expect("新连接的快照是新的，必须能写");

    // ③ 产品入口同样不受影响
    let store = Store::open(&path).expect("Store::open 不应把瞬时锁冲突抛给调用方");
    store.set_setting("product", "1").unwrap();

    // 四笔写入都应在库里
    let reader = Store::open(&path).unwrap();
    for key in ["warmup", "concurrent", "fresh", "product"] {
        assert!(
            reader.get_setting(key).unwrap().is_some(),
            "设置 {key} 丢失"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
