//! 轻量日志 —— 只做一件事：让"进程突然消失"留下痕迹。
//!
//! 为什么不用 `println!` / `eprintln!`：
//! release 版是 `#![windows_subsystem = "windows"]` 的 GUI 进程，**没有控制台**，
//! 标准流输出无处可去 —— 轻则整条日志丢失，重则写入失败的 `print!` 内部 panic
//! 把一次"记一笔日志"升级成"应用挂掉"。日志不能有自己的失败模式。
//!
//! （早期 release profile 里还配着 `panic = "abort"`，那会让上面这种 panic 直接
//! 终结进程 —— 现在这条配置已经拿掉了，理由写在 Cargo.toml 里。）
//!
//! 所以这里把日志落到应用数据目录的 `pristimer.log`，并额外做两件事：
//!   1. 超限即整体丢弃重写，日志不会无限增长；
//!   2. 装一个 panic hook。偶发崩溃最缺的不是复现步骤，而是"最后那句话"。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 单文件上限。8 MB 够记几千条，又不至于在用户磁盘里躺一个删不掉的大文件。
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 初始化。必须在任何 `log` 调用之前执行（`setup()` 的第一件事）。
pub fn init(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = LOG_PATH.set(dir.join("pristimer.log"));
    install_panic_hook();
}

/// 日志文件当前的落点；`init` 之前为 `None`。
///
/// 给「启动失败」那条路径用的：既然是让用户自己去看日志，
/// 就得把日志在哪儿说清楚，而不是让他猜。
pub fn path() -> Option<&'static Path> {
    LOG_PATH.get().map(PathBuf::as_path)
}

/// 追加一行日志。
///
/// **写失败一律静默放弃** —— 记日志这件事本身绝不能成为新的崩溃源，
/// 这正是我们要修掉的那类问题。
pub fn log(msg: impl AsRef<str>) {
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_BYTES {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{}] {}", utc_text(), msg.as_ref());
    }
}

fn utc_text() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format_utc(ms)
}

/// epoch 毫秒 → `YYYY-MM-DD HH:MM:SS`（UTC）。
///
/// 手写历法换算（Howard Hinnant 的 `civil_from_days`）而不是引入 chrono ——
/// 为一行时间戳加一个日期库不划算。标注 UTC 是为了不假装知道本地时区。
fn format_utc(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// panic hook：把 panic 的位置与载荷写进日志。
///
/// 只有当进程还活着的时候才写得到 —— 遗憾的是 `panic = "abort"` 下写文件
/// 的机会已经很紧张，所以这个 hook 的价值主要在**去掉 abort 之后**：
/// 那时 panic 变成可 unwind 的错误，日志能完整记录下现场。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "未知位置".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "非字符串 panic".to_string());
        log(format!("!!! PANIC @ {location} :: {payload}"));

        // release 下**故意不调用**默认 hook：它的职责是往 stderr 写回溯，而 GUI
        // 进程没有 stderr；一旦那次写入自身 panic，就会触发 "panic in panic"
        // 而被强制 abort —— 一个日志 hook 反过来杀死应用，正是要避免的事。
        // 调试构建下保留它，这样才能看到回溯。
        #[cfg(debug_assertions)]
        previous(info);
        #[cfg(not(debug_assertions))]
        let _ = &previous;
    }));
}
