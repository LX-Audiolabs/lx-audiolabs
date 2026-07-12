//! Optional coarse profiling helpers for UI hot paths.

use std::sync::atomic::{AtomicU64, Ordering};

/// Returns true if `SHARED_UI_PROFILE_TICKER` is set in the environment.
pub fn ticker_profile_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SHARED_UI_PROFILE_TICKER").is_ok())
}

/// Aggregate and print ticker timings every 120 calls.
pub fn report_ticker(tick_us: u64, total_us: u64) {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static TICK_US: AtomicU64 = AtomicU64::new(0);
    static TOTAL_US: AtomicU64 = AtomicU64::new(0);

    TICK_US.fetch_add(tick_us, Ordering::Relaxed);
    TOTAL_US.fetch_add(total_us, Ordering::Relaxed);
    let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if n.is_multiple_of(120) {
        let window = 120u64;
        let tick = TICK_US.swap(0, Ordering::Relaxed) as f64 / window as f64 / 1000.0;
        let total = TOTAL_US.swap(0, Ordering::Relaxed) as f64 / window as f64 / 1000.0;
        println!(
            "ticker_profile n={n:>5}  tick={tick:>7.3} ms  total={total:>7.3} ms"
        );
    }
}
