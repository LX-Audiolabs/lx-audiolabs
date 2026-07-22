//! Standalone Vizia profiler for shared canvas views.
//!
//! Stress-tests the Phase 2 optimizations by rendering several animated views
//! and updating them every frame.  Set `SHARED_UI_NO_LAYER_CACHE=1` to run the
//! same workload without retained layer caching for an A/B comparison.  Set
//! `SHARED_UI_PROFILE_LAYER_CACHE=1` to print per-call breakdown of static
//! recording vs picture replay vs dynamic overlay.
//!
//! Run:
//!     cargo run --release -p shared-ui --example profile
//! No cache:
//!     SHARED_UI_NO_LAYER_CACHE=1 cargo run --release -p shared-ui --example profile

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use vizia::prelude::*;

use shared_ui::{
    rgb, GoniometerView, SpectrumConfig, SpectrumCurve, SpectrumView, StereoMeterView,
};

// Wall-clock frame bookkeeping.
static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
static FRAME_INTERVAL_US: AtomicU64 = AtomicU64::new(0);
static UPDATE_US: AtomicU64 = AtomicU64::new(0);
static LAST_WALL: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone)]
struct Telemetry {
    peak_l: f32,
    peak_r: f32,
    hold_l: f32,
    hold_r: f32,
    balance: f32,
    phase_correlation: f32,
    gonio_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    gonio_write_pos: usize,
    spectrum: Vec<f32>,
    eq_curve: shared_ui::EqCurve,
}

fn main() {
    Application::new(|cx| {
        let sample_rate = 48000.0f32;
        let fft_size = 4096usize;
        let spectrum_bins = fft_size / 2;

        let gonio_samples = Arc::new(Mutex::new(vec![[0.0f32; 2]; 1024]));

        let telemetry = Signal::new(Telemetry {
            peak_l: -12.0,
            peak_r: -15.0,
            hold_l: -6.0,
            hold_r: -8.0,
            balance: 0.1,
            phase_correlation: 0.85,
            gonio_samples: gonio_samples.clone(),
            gonio_write_pos: 0,
            spectrum: vec![-90.0; spectrum_bins],
            eq_curve: shared_ui::EqCurve {
                points: (0..240)
                    .map(|i| {
                        let x = i as f32 / 239.0;
                        (x, (x * std::f32::consts::TAU).sin() * 6.0)
                    })
                    .collect(),
                min_db: -12.0,
                max_db: 12.0,
                line_color: rgb(1.0, 0.55, 0.15),
                fill_alpha: 0.0,
            },
        });

        VStack::new(cx, move |cx| {
            HStack::new(cx, move |cx| {
                Binding::new(cx, telemetry, move |cx| {
                    let t = telemetry.get();
                    StereoMeterView::new(
                        cx,
                        t.peak_l,
                        t.peak_r,
                        t.hold_l,
                        t.hold_r,
                        t.balance,
                    )
                    .width(Stretch(1.0))
                    .height(Stretch(1.0));
                });

                Binding::new(cx, telemetry, move |cx| {
                    let t = telemetry.get();
                    GoniometerView::new(
                        cx,
                        t.gonio_samples.clone(),
                        t.gonio_write_pos,
                        t.phase_correlation,
                    )
                    .width(Stretch(1.0))
                    .height(Stretch(1.0));
                });
            })
            .width(Stretch(1.0))
            .height(Pixels(220.0));

            // Four spectrum analysers tiled 2x2 to stress the smoothing and
            // Skia path generation enough that frame time becomes measurable.
            for row in 0..2 {
                HStack::new(cx, move |cx| {
                    for col in 0..2 {
                        let _ = (row, col);
                        Binding::new(cx, telemetry, move |cx| {
                            let t = telemetry.get();
                            SpectrumView::new(
                                cx,
                                SpectrumView {
                                    curves: vec![SpectrumCurve {
                                        spectrum: t.spectrum.clone(),
                                        color: rgb(0.1, 0.9, 0.7),
                                        fill_alpha: 0.18,
                                        line_alpha: 0.85,
                                        line_width: 1.6,
                                    }],
                                    config: SpectrumConfig {
                                        sample_rate,
                                        fft_size,
                                        ..Default::default()
                                    },
                                    resonance_peaks: vec![
                                        (120, 8.0),
                                        (400, 12.0),
                                        (900, 6.0),
                                    ],
                                    masking: vec![-90.0; spectrum_bins],
                                    eq_curve: Some(t.eq_curve.clone()),
                                    hovered_freq: std::cell::Cell::new(None),
                                },
                            )
                            .width(Stretch(1.0))
                            .height(Stretch(1.0));
                        });
                    }
                })
                .width(Stretch(1.0))
                .height(Stretch(1.0));
            }
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0));

        // Model holds the signal handles and is updated from on_idle via events.
        ProfileModel::default().build(cx);
        cx.emit(ProfileEvent::Init { telemetry, gonio_samples });
    })
    .inner_size((900, 700))
    .on_idle(|cx| {
        let now = Instant::now();
        let mut last = LAST_WALL.lock().unwrap();
        if let Some(prev) = *last {
            let interval_us = prev.elapsed().as_micros() as u64;
            FRAME_INTERVAL_US.fetch_add(interval_us, Ordering::Relaxed);
        }
        *last = Some(now);
        drop(last);

        let update_start = Instant::now();
        cx.emit(ProfileEvent::Tick);
        let update_us = update_start.elapsed().as_micros() as u64;
        UPDATE_US.fetch_add(update_us, Ordering::Relaxed);

        let count = FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(120) {
            let total_interval = FRAME_INTERVAL_US.swap(0, Ordering::Relaxed) as f64 / 1000.0;
            let total_update = UPDATE_US.swap(0, Ordering::Relaxed) as f64 / 1000.0;
            let avg_interval = total_interval / 120.0;
            let avg_update = total_update / 120.0;
            println!(
                "frames={count:>5}  avg interval={avg_interval:>7.3} ms  update={avg_update:>7.3} ms"
            );
        }
    })
    .run();
}

#[derive(Debug)]
enum ProfileEvent {
    Init {
        telemetry: Signal<Telemetry>,
        gonio_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    },
    Tick,
}

#[derive(Default)]
struct ProfileModel {
    telemetry: Option<Signal<Telemetry>>,
    gonio_samples: Option<Arc<Mutex<Vec<[f32; 2]>>>>,
    frame: u64,
}

impl Model for ProfileModel {
    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|e, _| match e {
            ProfileEvent::Init { telemetry, gonio_samples } => {
                self.telemetry = Some(*telemetry);
                self.gonio_samples = Some(gonio_samples.clone());
            }
            ProfileEvent::Tick => {
                let Some(telemetry) = self.telemetry else { return };
                let Some(gonio_samples) = self.gonio_samples.as_ref() else { return };

                self.frame += 1;
                let f = self.frame as f32;

                let mut prev = telemetry.get();

                prev.peak_l = -20.0 + (f * 0.05).sin() * 18.0;
                prev.peak_r = -22.0 + (f * 0.07 + 1.0).sin() * 16.0;
                prev.hold_l = prev.peak_l.max(-6.0);
                prev.hold_r = prev.peak_r.max(-6.0);
                prev.balance = (f * 0.02).sin();
                prev.phase_correlation = 0.5 + 0.4 * (f * 0.03).sin();

                if let Ok(mut samples) = gonio_samples.lock() {
                    let n = samples.len();
                    let wp = prev.gonio_write_pos % n;
                    samples[wp] = [
                        (f * 0.1).sin() * 0.6,
                        (f * 0.13 + 0.5).sin() * 0.5,
                    ];
                    prev.gonio_write_pos = (wp + 1) % n;
                    prev.gonio_samples = gonio_samples.clone();
                }

                let bins = prev.spectrum.len();
                for (i, db) in prev.spectrum.iter_mut().enumerate() {
                    let bin_f = i as f32 / bins.max(1) as f32;
                    let noise = (f * 0.2 + bin_f * 10.0).sin() * 8.0;
                    *db = -70.0 + noise + (1.0 - bin_f) * 15.0;
                }

                for (x, db) in prev.eq_curve.points.iter_mut() {
                    *db = (f * 0.03 + *x * std::f32::consts::TAU).sin() * 6.0;
                }

                telemetry.set(prev);
            }
        });
    }
}
