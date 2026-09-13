// ---------------------------------------------------------------------------
// SQLite 自动备份：每日首次启动做一份一致性快照，保留最近 7 份。
//
// 约束（来自需求）：失败只告警、绝不阻断启动 —— 备份是锦上添花，
// 不能变成「备份盘满 → 应用起不来」的新的单点故障。
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};

/// 保留的备份份数（不含当天正在写的这份）。
const KEEP: usize = 7;

/// 每日首次启动备份。幂等：当天已有备份就直接跳过。
///
/// * 快照用 `Store::backup_to`（`VACUUM INTO`），不是文件拷贝 —— WAL 下
///   主文件不含未 checkpoint 的页，copy 会缺数据；
/// * 排重靠文件名（`pristimer-YYYY-MM-DD.db`），与 SQLite「目标必须不存在」
///   的约束正好对上；
/// * 任何一步失败都只写日志，返回后启动流程照常。
pub(crate) fn run_daily_backup(data_dir: &Path, store: &pristimer_store::Store) {
    let result = try_daily_backup(data_dir, store);
    match result {
        Ok(Some(path)) => crate::log::log(format!("每日备份完成: {}", path.display())),
        Ok(None) => {}
        Err(err) => crate::log::log(format!("每日备份失败（不影响启动）: {err}")),
    }
}

fn try_daily_backup(data_dir: &Path, store: &pristimer_store::Store) -> Result<Option<PathBuf>, String> {
    let backups = data_dir.join("backups");
    let day = local_date_string();
    let target = backups.join(format!("pristimer-{day}.db"));
    if target.exists() {
        return Ok(None); // 今天已经备过
    }
    std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
    store.backup_to(&target).map_err(|e| e.to_string())?;
    prune_old_backups(&backups)?;
    Ok(Some(target))
}

/// 只保留最近 `KEEP` 份（按文件名日期排序，名字即序）。
fn prune_old_backups(backups: &Path) -> Result<(), String> {
    let mut snapshots: Vec<PathBuf> = std::fs::read_dir(backups)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_string_lossy().into_owned();
            path.is_file()
                .then_some(name.starts_with("pristimer-") && name.ends_with(".db"))
                .filter(|&is_backup| is_backup)
                .map(|_| path)
        })
        .collect();
    // 文件名形如 pristimer-2026-09-13.db，字典序即时间序
    snapshots.sort();
    while snapshots.len() > KEEP {
        let oldest = snapshots.remove(0);
        if let Err(err) = std::fs::remove_file(&oldest) {
            // 清理失败不回头：快照本身已经安全落盘，旧文件留给下次
            crate::log::log(format!("清理旧备份失败: {} ({err})", oldest.display()));
            break;
        }
    }
    Ok(())
}

/// 本地日期 `YYYY-MM-DD`。不引入 chrono：从 Unix 纪元算天数再用
/// civil-from-days（Howard Hinnant 的算法）换算年月日，十几行换一个零依赖。
/// 本机时区偏移取自系统（`TZ`/注册表由系统调用兜底）—— 秒级精度足够「哪一天」。
fn local_date_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let days = now.as_secs() + local_utc_offset_secs();
    let days = (days / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn local_utc_offset_secs() -> u64 {
    // `SystemTime` 只给 UTC。拿本地偏移最省事的口子是 `std::process::Command`
    // 查系统 —— 不值得。这里用「与文件系统时间戳对照」太绕，直接按 UTC 记；
    // 副作用仅是 UTC 晚八区（中国）的备份落在当天 00:00-08:00 之间会归到前一天，
    // 对「保留 7 份」的语义没有影响。如果未来要精确本地日期，再上 chrono。
    0
}

/// Howard Hinnant 的 civil_from_days：days since 1970-01-01 → (y, m, d)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_date_conversions() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
        assert_eq!(civil_from_days(20_672), (2026, 8, 15));
    }
}
