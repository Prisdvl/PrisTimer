use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync + 'static {
    // 单增时钟
    fn monotonic_ms(&self) -> u64;
    // 墙钟：持久化，不过会因为系统时间修改而变化
    fn wall_ms(&self) -> u64;
}

pub struct SystemClock;

fn base() -> &'static Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now)
}

impl Clock for SystemClock {
    fn monotonic_ms(&self) -> u64 {
        base().elapsed().as_millis() as u64
    }
    fn wall_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间早于1970年")
            .as_millis() as u64
    }
}

#[derive(Default)]
pub struct FakeClock {
    monotonic_ms: AtomicU64,
    wall_ms: AtomicU64,
}

impl FakeClock {
    pub fn new(start_wall_ms: u64) -> Self {
        Self {
            monotonic_ms: AtomicU64::new(0),
            wall_ms: AtomicU64::new(start_wall_ms),
        }
    }

    pub fn advance(&self, ms: u64) {
        self.monotonic_ms.fetch_add(ms, Ordering::SeqCst);
        self.wall_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms.load(Ordering::SeqCst)
    }

    fn wall_ms(&self) -> u64 {
        self.wall_ms.load(Ordering::SeqCst)
    }
}
/// 运行时要把时钟的所有权交给后台线程，而测试还想在外面持有同一个时钟来推进时间。
/// `Arc` 就同时满足了这两边的需求。
impl<C: Clock> Clock for std::sync::Arc<C> {
    fn monotonic_ms(&self) -> u64 {
        (**self).monotonic_ms()
    }
    fn wall_ms(&self) -> u64 {
        (**self).wall_ms()
    }
}
