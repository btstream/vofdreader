//! Opt-in reproducible workload. Ordinary sessions have no extra timer/input.
use crate::*;

impl ReaderApp {
    pub(crate) fn start_scroll_benchmark(cx: &mut Context<Self>) {
        let Some(steps) = std::env::var("OFD_SCROLL_BENCH_STEPS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|steps| (1..=300).contains(steps))
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let mut ready = false;
            for _ in 0..100 {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let state = this.update(cx, |this, _| this.active.renderer.is_some());
                match state {
                    Ok(true) => {
                        ready = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
            if !ready {
                eprintln!("[scroll-bench] invalid: no document ready");
                return;
            }
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let Ok(Some((generation, baseline))) = this.update(cx, |this, cx| {
                if this.view_mode != ViewMode::Continuous || this.grid_open {
                    return None;
                }
                this.go_to_page(0, cx);
                Some((
                    this.active.generation,
                    this.gpu.borrow().stats.background_jobs,
                ))
            }) else {
                eprintln!("[scroll-bench] invalid: requires continuous view");
                return;
            };
            let started = Instant::now();
            eprintln!("[scroll-bench] begin steps={steps} delta_px=1000 interval_ms=200");
            for direction in [1.0, -1.0] {
                for _ in 0..steps {
                    if !matches!(
                        this.update(cx, |this, cx| {
                            if this.active.generation != generation
                                || this.view_mode != ViewMode::Continuous
                            {
                                return false;
                            }
                            this.scroll_continuous(1000.0 * direction, cx);
                            true
                        }),
                        Ok(true)
                    ) {
                        return;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                }
            }
            let stopped_jobs = this
                .update(cx, |this, _| this.gpu.borrow().stats.background_jobs)
                .unwrap_or(baseline);
            eprintln!(
                "[scroll-bench] stopped elapsed_ms={} jobs={}",
                started.elapsed().as_millis(),
                stopped_jobs.saturating_sub(baseline)
            );
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |this, _| {
                if this.active.generation != generation {
                    return;
                }
                let thumbs: usize = this
                    .active
                    .thumbs
                    .iter()
                    .flatten()
                    .map(|image| {
                        let size = image.size(0);
                        size.width.0 as usize * size.height.0 as usize * 4
                    })
                    .sum();
                eprintln!(
                    "[scroll-bench] settled {} thumbs={:.1}MiB jobs_since_stop={}",
                    this.budget_diagnostic(),
                    thumbs as f64 / 1048576.0,
                    this.gpu
                        .borrow()
                        .stats
                        .background_jobs
                        .saturating_sub(stopped_jobs)
                );
                #[cfg(target_os = "linux")]
                if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                    eprintln!(
                        "[scroll-bench] {}",
                        status
                            .lines()
                            .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    );
                }
            });
        })
        .detach();
    }
}
