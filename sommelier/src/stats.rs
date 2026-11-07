/*
Copyright 2026 Google LLC

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

     https://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

//! Lightweight traffic/frame statistics behind mainline's observability flags.
//!
//! Mainline sommelier wires these to Perfetto. Pulling in a trace engine would
//! be disproportionate here, so the same CLI surface produces plain, greppable
//! output instead:
//!
//! * `--timing-filename` and `--stats-log` — CSV samples, one row per
//!   `--stats-timer` interval plus a final row when a client disconnects.
//! * `--stats-summary` — a human-readable snapshot, rewritten on every tick so
//!   the file always reflects the live process.
//! * `--trace-filename` — one line per proxied Wayland message with direction,
//!   interface, opcode and size; `--trace-system` adds a header with host and
//!   process details.
//!
//! All writes go through a single [`Stats`] shared by every client task, so a
//! `--parent` process produces one coherent stream rather than one file per
//! connection. Nothing is written unless at least one flag is present.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Resolved subset of the observability CLI flags.
#[derive(Debug, Clone, Default)]
pub struct StatsConfig {
    pub timing_filename: Option<PathBuf>,
    pub stats_log: Option<PathBuf>,
    pub stats_summary: Option<PathBuf>,
    /// Sampling period in milliseconds. Mainline's `--stats-timer` is in
    /// *seconds* and multiplies by 1000 internally (sommelier.cc:3757, :4353).
    pub stats_timer_ms: u64,
    pub trace_filename: Option<PathBuf>,
    pub trace_system: bool,
}

impl StatsConfig {
    pub fn enabled(&self) -> bool {
        self.timing_filename.is_some()
            || self.stats_log.is_some()
            || self.stats_summary.is_some()
            || self.trace_filename.is_some()
    }

    pub fn period(&self) -> std::time::Duration {
        // A zero/unset timer still needs a wake-up so the summary stays fresh.
        std::time::Duration::from_millis(self.stats_timer_ms.clamp(100, 3_600_000))
    }
}

#[derive(Debug, Default)]
struct Counters {
    client_requests: AtomicU64,
    host_events: AtomicU64,
    client_bytes: AtomicU64,
    host_bytes: AtomicU64,
    frames: AtomicU64,
}

/// Immutable snapshot used by both the CSV writer and the summary writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub elapsed_ms: u64,
    pub client_requests: u64,
    pub host_events: u64,
    pub client_bytes: u64,
    pub host_bytes: u64,
    pub frames: u64,
}

impl Snapshot {
    pub fn req_per_s(&self) -> f64 {
        rate(self.client_requests, self.elapsed_ms)
    }

    pub fn evt_per_s(&self) -> f64 {
        rate(self.host_events, self.elapsed_ms)
    }
}

fn rate(count: u64, elapsed_ms: u64) -> f64 {
    if elapsed_ms == 0 {
        0.0
    } else {
        count as f64 * 1000.0 / elapsed_ms as f64
    }
}

const CSV_HEADER: &str =
    "elapsed_ms,client_requests,host_events,client_bytes,host_bytes,frames,req_per_s,evt_per_s\n";

fn create(path: &Path) -> std::io::Result<BufWriter<File>> {
    ensure_parent(path)?;
    Ok(BufWriter::new(File::create(path)?))
}

fn append(path: &Path) -> std::io::Result<(BufWriter<File>, bool)> {
    ensure_parent(path)?;
    let needs_header = !std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    Ok((
        BufWriter::new(OpenOptions::new().create(true).append(true).open(path)?),
        needs_header,
    ))
}

fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Stats {
    cfg: StatsConfig,
    start: Instant,
    counters: Counters,
    csv: Mutex<Vec<BufWriter<File>>>,
    trace: Option<Mutex<BufWriter<File>>>,
}

impl Stats {
    pub fn new(cfg: StatsConfig) -> Arc<Self> {
        let mut csv = Vec::new();
        // `--timing-filename` starts a fresh log each run; `--stats-log`
        // appends across runs (sommelier.cc:3751 "log/append frame timing
        // stats"), so only a brand-new file gets a CSV header.
        if let Some(path) = &cfg.timing_filename {
            match create(path) {
                Ok(mut w) => {
                    let _ = w.write_all(CSV_HEADER.as_bytes());
                    let _ = w.flush();
                    csv.push(w);
                }
                Err(e) => log::warn!("cannot open stats file {}: {}", path.display(), e),
            }
        }
        if let Some(path) = &cfg.stats_log {
            match append(path) {
                Ok((mut w, needs_header)) => {
                    if needs_header {
                        let _ = w.write_all(CSV_HEADER.as_bytes());
                        let _ = w.flush();
                    }
                    csv.push(w);
                }
                Err(e) => log::warn!("cannot open stats log {}: {}", path.display(), e),
            }
        }

        let trace = cfg
            .trace_filename
            .as_ref()
            .and_then(|path| match create(path) {
                Ok(mut w) => {
                    let _ = writeln!(w, "# sommelier-rs-r message trace");
                    if cfg.trace_system {
                        let _ =
                            writeln!(w, "# pid={} started_us={}", std::process::id(), unix_us());
                        let _ = writeln!(w, "# transport=virtio_wl_or_local");
                    }
                    let _ = w.flush();
                    Some(Mutex::new(w))
                }
                Err(e) => {
                    log::warn!("cannot open trace file {}: {}", path.display(), e);
                    None
                }
            });

        // Write an initial summary so the file exists as soon as the service
        // reports READY=1, not only after the first timer tick.
        let stats = Arc::new(Self {
            cfg,
            start: Instant::now(),
            counters: Counters::default(),
            csv: Mutex::new(csv),
            trace,
        });
        stats.write_summary();
        stats
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled()
    }

    pub fn period(&self) -> std::time::Duration {
        self.cfg.period()
    }

    /// Record one proxied Wayland message. `is_frame` is true for
    /// `wl_surface.frame` requests, which mainline counts as frame pacing.
    #[inline]
    pub fn record(
        &self,
        client_to_host: bool,
        interface: Option<&str>,
        opcode: u16,
        len: usize,
        is_frame: bool,
    ) {
        if !self.enabled() {
            return;
        }
        let c = &self.counters;
        if client_to_host {
            c.client_requests.fetch_add(1, Ordering::Relaxed);
            c.client_bytes.fetch_add(len as u64, Ordering::Relaxed);
        } else {
            c.host_events.fetch_add(1, Ordering::Relaxed);
            c.host_bytes.fetch_add(len as u64, Ordering::Relaxed);
        }
        if is_frame {
            c.frames.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(trace) = &self.trace {
            if let Ok(mut w) = trace.lock() {
                let _ = writeln!(
                    w,
                    "{} {} {} {} {}",
                    unix_us(),
                    if client_to_host { "C2H" } else { "H2C" },
                    interface.unwrap_or("?"),
                    opcode,
                    len
                );
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let c = &self.counters;
        Snapshot {
            elapsed_ms: self.start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            client_requests: c.client_requests.load(Ordering::Relaxed),
            host_events: c.host_events.load(Ordering::Relaxed),
            client_bytes: c.client_bytes.load(Ordering::Relaxed),
            host_bytes: c.host_bytes.load(Ordering::Relaxed),
            frames: c.frames.load(Ordering::Relaxed),
        }
    }

    /// Append one CSV row to every configured timing/stats log.
    pub fn sample(&self) {
        if !self.enabled() {
            return;
        }
        let s = self.snapshot();
        if let Ok(mut writers) = self.csv.lock() {
            for w in writers.iter_mut() {
                let _ = writeln!(
                    w,
                    "{},{},{},{},{},{},{:.2},{:.2}",
                    s.elapsed_ms,
                    s.client_requests,
                    s.host_events,
                    s.client_bytes,
                    s.host_bytes,
                    s.frames,
                    s.req_per_s(),
                    s.evt_per_s()
                );
                let _ = w.flush();
            }
        }
        self.write_summary();
        // Flush the trace on every sample so a `kill -9` (or a crashed
        // compositor) still leaves the messages received up to the last tick.
        self.flush_trace();
    }

    fn flush_trace(&self) {
        if let Some(trace) = &self.trace {
            if let Ok(mut w) = trace.lock() {
                let _ = w.flush();
            }
        }
    }

    fn write_summary(&self) {
        let Some(path) = &self.cfg.stats_summary else {
            return;
        };
        let s = self.snapshot();
        // `create` truncates, so the file is always a snapshot rather than an
        // unbounded history (mainline's --stats-summary is a summary too).
        let Ok(mut w) = create(path) else {
            return;
        };
        let _ = writeln!(w, "sommelier-rs-r traffic summary");
        let _ = writeln!(w, "elapsed_ms:      {}", s.elapsed_ms);
        let _ = writeln!(w, "client_requests: {}", s.client_requests);
        let _ = writeln!(w, "host_events:     {}", s.host_events);
        let _ = writeln!(w, "client_bytes:    {}", s.client_bytes);
        let _ = writeln!(w, "host_bytes:      {}", s.host_bytes);
        let _ = writeln!(w, "frames:          {}", s.frames);
        let _ = writeln!(w, "req_per_s:       {:.2}", s.req_per_s());
        let _ = writeln!(w, "evt_per_s:       {:.2}", s.evt_per_s());
        let _ = w.flush();
    }

    /// Final sample when a client disconnects (or the process winds down).
    pub fn finish(&self) {
        self.sample();
    }
}

fn unix_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sommelier-rs-r-stats-{}-{}",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn disabled_config_writes_nothing() {
        let cfg = StatsConfig::default();
        assert!(!cfg.enabled());
        let stats = Stats::new(cfg);
        stats.record(true, Some("wl_display"), 1, 8, false);
        assert_eq!(stats.snapshot().client_requests, 0);
    }

    #[test]
    fn counts_and_csv_and_summary_are_consistent() {
        let dir = tmp("all");
        let _ = std::fs::remove_dir_all(&dir);
        let csv = dir.join("timing.csv");
        let summary = dir.join("summary.txt");
        let trace = dir.join("trace.log");
        let stats = Stats::new(StatsConfig {
            timing_filename: Some(csv.clone()),
            stats_summary: Some(summary.clone()),
            trace_filename: Some(trace.clone()),
            trace_system: true,
            stats_timer_ms: 100,
            ..Default::default()
        });
        stats.record(true, Some("wl_surface"), 3, 16, true);
        stats.record(false, Some("wl_output"), 2, 24, false);
        stats.sample();

        let s = stats.snapshot();
        assert_eq!(s.client_requests, 1);
        assert_eq!(s.host_events, 1);
        assert_eq!(s.client_bytes, 16);
        assert_eq!(s.host_bytes, 24);
        assert_eq!(s.frames, 1);

        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(text.starts_with("elapsed_ms,"));
        assert_eq!(text.lines().count(), 2);
        // The row's elapsed_ms is sampled when it is written, so it can be a
        // millisecond or two older than the later `snapshot()` above. Assert
        // on the stable columns instead of the timestamp.
        let row = text.lines().nth(1).unwrap();
        assert!(row.split(',').next().unwrap().parse::<u64>().is_ok());
        assert!(row.contains(",1,1,16,24,1,"));

        let summary_text = std::fs::read_to_string(&summary).unwrap();
        assert!(summary_text.contains("client_requests: 1"));

        let trace_text = std::fs::read_to_string(&trace).unwrap();
        assert!(trace_text.contains("C2H wl_surface 3 16"));
        assert!(trace_text.contains("H2C wl_output 2 24"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_is_a_snapshot_not_a_log() {
        let path = tmp("summary.txt");
        let _ = std::fs::remove_file(&path);
        let stats = Stats::new(StatsConfig {
            stats_summary: Some(path.clone()),
            ..Default::default()
        });
        stats.record(true, None, 0, 8, false);
        stats.sample();
        stats.record(true, None, 0, 8, false);
        stats.sample();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("client_requests: 2"));
        assert_eq!(text.matches("traffic summary").count(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stats_log_appends_across_runs_with_one_header() {
        let path = tmp("append.csv");
        let _ = std::fs::remove_file(&path);
        for _ in 0..2 {
            let stats = Stats::new(StatsConfig {
                stats_log: Some(path.clone()),
                ..Default::default()
            });
            stats.record(true, None, 0, 8, false);
            stats.sample();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("elapsed_ms,").count(), 1);
        // header + one row per run
        assert_eq!(text.lines().count(), 3);
        let _ = std::fs::remove_file(&path);
    }
}
