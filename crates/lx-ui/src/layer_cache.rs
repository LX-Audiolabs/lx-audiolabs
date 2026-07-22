//! Retained layer cache for static view backgrounds using Skia Pictures.
//!
//! Vizia rebuilds the animated views every tick because they live inside
//! `Binding::new(cx, telemetry, ...)`.  That destroys any per-view state, so
//! the cache lives in a thread-local map keyed by physical size and an
//! optional static-hash.  The cached Skia `Picture` records vector drawing
//! commands, not raster pixels, so it is cheap to replay and stays sharp on
//! HiDPI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread::LocalKey;
use std::time::Instant;

use vizia::prelude::*;
use vizia::vg;

fn layer_cache_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var("LX_UI_NO_LAYER_CACHE").is_ok())
}

fn layer_cache_profile() -> bool {
    static PROFILE: OnceLock<bool> = OnceLock::new();
    *PROFILE.get_or_init(|| std::env::var("LX_UI_PROFILE_LAYER_CACHE").is_ok())
}

static PROFILE_CALLS: AtomicU64 = AtomicU64::new(0);
static PROFILE_STATIC_US: AtomicU64 = AtomicU64::new(0);
static PROFILE_DYNAMIC_US: AtomicU64 = AtomicU64::new(0);
static PROFILE_REPLAY_US: AtomicU64 = AtomicU64::new(0);

fn report_profile(static_us: u64, dynamic_us: u64, replay_us: u64) {
    PROFILE_STATIC_US.fetch_add(static_us, Ordering::Relaxed);
    PROFILE_DYNAMIC_US.fetch_add(dynamic_us, Ordering::Relaxed);
    PROFILE_REPLAY_US.fetch_add(replay_us, Ordering::Relaxed);
    let n = PROFILE_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if n.is_multiple_of(120) {
        let window = 120u64;
        let s = PROFILE_STATIC_US.swap(0, Ordering::Relaxed) as f64 / window as f64 / 1000.0;
        let d = PROFILE_DYNAMIC_US.swap(0, Ordering::Relaxed) as f64 / window as f64 / 1000.0;
        let r = PROFILE_REPLAY_US.swap(0, Ordering::Relaxed) as f64 / window as f64 / 1000.0;
        println!(
            "layer_cache n={n:>5}  static={s:>7.3} ms  dynamic={d:>7.3} ms  replay={r:>7.3} ms"
        );
    }
}

/// Key for a cached static layer.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerCacheKey {
    /// Physical width in pixels.
    pub width: u32,
    /// Physical height in pixels.
    pub height: u32,
    /// Scale factor multiplied by 100 to keep it integer.
    pub scale_x100: u32,
    /// Hash of anything else that affects the static background (grid labels,
    /// colours, etc.).  Zero when only size matters.
    pub static_hash: u64,
}

impl LayerCacheKey {
    pub fn from_bounds(cx: &mut DrawContext, static_hash: u64) -> Self {
        let b = cx.bounds();
        // Vizia's cache bounds are already in physical pixels, so use them
        // directly.  Keeping the scale factor in the key ensures we re-record
        // if the backing scale changes while the reported bounds haven't yet
        // caught up during a layout transition.
        Self {
            width: b.width().round().max(1.0) as u32,
            height: b.height().round().max(1.0) as u32,
            scale_x100: (cx.scale_factor() * 100.0).round().max(1.0) as u32,
            static_hash,
        }
    }
}

pub type LayerCache = LocalKey<RefCell<HashMap<LayerCacheKey, vg::Picture>>>;

/// Records the static background of a view to a Skia `Picture` and replays it,
/// then calls `paint_dynamic` for the moving overlay.
///
/// `cache` is a thread-local `LayerCache` declared by the view.
/// `static_hash` should cover everything that changes the background but does
/// not change every frame (grid lines, labels, colours).
pub fn draw_cached_layer(
    cache: &'static LayerCache,
    cx: &mut DrawContext,
    canvas: &vg::Canvas,
    static_hash: u64,
    paint_static: impl FnOnce(&vg::Canvas),
    paint_dynamic: impl FnOnce(&vg::Canvas),
) {
    let key = LayerCacheKey::from_bounds(cx, static_hash);

    let profile = layer_cache_profile();

    let cached = if layer_cache_disabled() {
        None
    } else {
        cache.with(|m| m.borrow().get(&key).cloned())
    };

    let t0 = if profile { Some(Instant::now()) } else { None };
    let picture = match cached {
        Some(p) => p,
        None => {
            let phys_w = key.width as f32;
            let phys_h = key.height as f32;
            let mut recorder = vg::PictureRecorder::new();
            let rec_canvas = recorder.begin_recording(
                vg::Rect::new(0.0, 0.0, phys_w, phys_h),
                false,
            );
            // Record in the view's local coordinate system (the caller has
            // already translated the real canvas so (0,0) is the view origin).
            paint_static(rec_canvas);
            let picture = recorder.finish_recording_as_picture(None).expect("picture recorder");
            cache.with(|m| m.borrow_mut().insert(key, picture.clone()));
            picture
        }
    };
    let static_us = t0.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);

    let t1 = if profile { Some(Instant::now()) } else { None };
    // Replay the recorded picture.  The real canvas is already translated to
    // the view origin, so drawing the picture at (0,0) places it correctly.
    canvas.draw_picture(&picture, None, None);
    let replay_us = t1.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);

    let t2 = if profile { Some(Instant::now()) } else { None };
    paint_dynamic(canvas);
    let dynamic_us = t2.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);

    if profile {
        report_profile(static_us, dynamic_us, replay_us);
    }
}

/// Convenience macro to declare a thread-local layer cache for a view.
#[macro_export]
macro_rules! declare_layer_cache {
    ($name:ident) => {
        thread_local! {
            static $name: std::cell::RefCell<std::collections::HashMap<
                $crate::layer_cache::LayerCacheKey,
                vizia::vg::Picture,
            >> = std::cell::RefCell::new(std::collections::HashMap::new());
        }
    };
}
