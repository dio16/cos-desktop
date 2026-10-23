//! Explicit-feature Windows fixture observer. No frame requests, Runtime operations,
//! private GPUI access, user payloads or S3 statistics. All I/O stays on one worker.

#[path = "../examples/r0_owner/sha256.rs"]
mod sha256;

use desktop_gpui::r0_probe::{self, Control, Event, EventKind};
use gpui::profiler::journal::{
    ForegroundEvent, ForegroundJournalCollector, ForegroundJournalEntry, FrameStateChange,
    IntervalBoundary,
};
use gpui::{App, FrameTiming, PresentTiming};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const GPUI_PIN: &str = "395def7a067b32ed598fcb3f048969ada9b54942";
const QUEUE: usize = 2048;
const RETAINED: u64 = 256;
const BYTES: usize = 1024 * 1024;
const WINDOWS: usize = 16;
const DURATION_NS: u64 = 30_000_000_000;
const DRAIN: Duration = Duration::from_millis(500);
const SHUTDOWN: Duration = Duration::from_secs(1);
const RUNNING: u8 = 1;
const COMMITTED: u8 = 2;
const FAILED: u8 = 3;
const UNCONFIRMED: u8 = 4;

#[derive(Clone, Default)]
pub struct Report(Arc<AtomicU8>);

impl Report {
    pub fn committed(&self) -> bool {
        self.0.load(Ordering::Acquire) == COMMITTED
    }
}

pub fn install(cx: &mut App, report: Report) -> io::Result<()> {
    let origin = Instant::now();
    let collector = cx.foreground_journal().collector();
    let (sender, receiver) = mpsc::sync_channel(QUEUE);
    let control = Arc::new(Control::new(origin));
    let lease = r0_probe::attach(sender, Arc::clone(&control)).map_err(io::Error::other)?;
    let worker_control = Arc::clone(&control);
    let worker_report = report.clone();
    report.0.store(RUNNING, Ordering::Release);
    std::thread::Builder::new()
        .name("r0-s2b-export".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                capture(collector, receiver, origin, worker_control, &worker_report)
            }));
            let (category, failed) = match outcome {
                Ok(Ok(())) => (None, false),
                Ok(Err(error)) => (Some(format!("{:?}\n", error.kind())), true),
                Err(_) => (Some("panic\n".to_owned()), true),
            };
            if let (Some(category), Some(directory)) = (
                category,
                std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            ) {
                // Development-only closed category; never a path, payload or OS text.
                let _ = std::fs::write(directory.join("r0-s2b-error.txt"), category);
            }
            if failed {
                let _ = worker_report.0.compare_exchange(
                    RUNNING,
                    FAILED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        })?;
    cx.on_app_quit(move |cx| {
        lease.stop();
        let report = report.clone();
        // This wait runs on the background executor, never on the GUI thread.
        cx.background_executor().spawn(async move {
            let deadline = Instant::now() + SHUTDOWN;
            while report.0.load(Ordering::Acquire) == RUNNING && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = report.0.compare_exchange(
                RUNNING,
                UNCONFIRMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        })
    })
    .detach();
    Ok(())
}

fn ns(origin: Instant, at: Instant) -> io::Result<u64> {
    at.checked_duration_since(origin)
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
        .ok_or_else(|| io::ErrorKind::InvalidData.into())
}

/// The S2A wrapper also holds independent full-chain Pins throughout this lifetime.
/// Open each component without following reparse points and retain every handle.
fn pin_directory(path: &Path) -> io::Result<Vec<File>> {
    let mut parts = path.components();
    if !matches!(parts.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
        || parts.next() != Some(Component::RootDir)
        || parts.any(|p| !matches!(p, Component::Normal(n) if !n.to_string_lossy().contains(':')))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut chain: Vec<_> = path.ancestors().collect();
    chain.reverse();
    let mut held = Vec::with_capacity(chain.len());
    for component in chain {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x0220_0000)
            .open(component)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        held.push(file);
    }
    Ok(held)
}

fn marker(path: &Path, expected: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 || metadata.len() != 32 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = [0u8; 32];
    file.read_exact(&mut bytes)?;
    if bytes != expected.as_bytes() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

struct LimitedWriter {
    file: File,
    written: usize,
}

impl Write for LimitedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > BYTES.saturating_sub(self.written) {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let count = self.file.write(data)?;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Default)]
struct Record {
    // 0 input, 1 dirty, 2 CPU draw, 3 platform submission, 4 close,
    // 5 source gap, 6 idle, 7 power boundary, 8 local mutation, 9 actual notify.
    kind: usize,
    window: Option<u64>,
    lifetime: Option<u64>,
    start: Option<u64>,
    end: Option<u64>,
    dirty: Option<u64>,
    invalidations: Option<u64>,
    pair_start: Option<u64>,
    pair_end: Option<u64>,
    category: Option<u64>,
    caused_invalidation: Option<bool>,
    lost: Option<u64>,
}

struct Nullable<T>(Option<T>);
impl<T: std::fmt::Display> std::fmt::Display for Nullable<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("null"),
        }
    }
}

impl Record {
    fn write(&self, out: &mut impl Write, seq: u64) -> io::Result<()> {
        write!(
            out,
            "{{\"seq\":{seq},\"kind\":{},\"window\":{},\"lifetime\":{},\"start_ns\":{},\"end_ns\":{},\"dirty_ns\":{},\"invalidations\":{},\"paired_draw_start_ns\":{},\"paired_draw_end_ns\":{},\"category\":{},\"caused_invalidation\":{},\"lost\":{}}}",
            self.kind,
            Nullable(self.window),
            Nullable(self.lifetime),
            Nullable(self.start),
            Nullable(self.end),
            Nullable(self.dirty),
            Nullable(self.invalidations),
            Nullable(self.pair_start),
            Nullable(self.pair_end),
            Nullable(self.category),
            Nullable(self.caused_invalidation),
            Nullable(self.lost)
        )
    }
}

#[derive(Default)]
struct Counts {
    journal_lost: u64,
    journal_seen: u64,
    local_received: u64,
    eligible: u64,
    retained: u64,
    excluded: u64,
    pre_scope: u64,
    post_scope: u64,
    invalid_clock: u64,
    barrier_spans: u64,
    correlation_invalid: u64,
    input_unassociated: u64,
    unmatched_submission: u64,
    undrained_journal: u64,
    kinds: [u64; 10],
}

struct Observer {
    origin: Instant,
    start: u64,
    end: u64,
    barrier: u64,
    // WindowId already includes the source's opaque identity. Do not decode it.
    windows: Vec<(u64, Option<u64>)>,
    counts: Counts,
}

impl Observer {
    fn point(&mut self, at: Instant) -> io::Result<u64> {
        let value = match ns(self.origin, at) {
            Ok(value) => value,
            Err(error) => {
                self.counts.invalid_clock += 1;
                return Err(error);
            }
        };
        if value < self.start {
            self.counts.pre_scope += 1;
            return Err(io::ErrorKind::InvalidData.into());
        }
        if value > self.end {
            self.counts.post_scope += 1;
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(value)
    }

    fn span(&mut self, start: Instant, end: Instant) -> io::Result<(u64, u64)> {
        if end < start {
            self.counts.invalid_clock += 1;
            return Err(io::ErrorKind::InvalidData.into());
        }
        let (start, end) = (self.point(start)?, self.point(end)?);
        if start < self.barrier {
            self.counts.barrier_spans += 1;
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok((start, end))
    }

    fn window(&mut self, key: u64, at: u64, closing: bool) -> io::Result<u64> {
        let index = match self.windows.iter().position(|(id, _)| *id == key) {
            Some(index)
                if self.windows[index]
                    .1
                    .is_none_or(|closed| !closing && at <= closed) =>
            {
                index
            }
            Some(_) => {
                self.counts.correlation_invalid += 1;
                return Err(io::ErrorKind::InvalidData.into());
            }
            None if self.windows.len() < WINDOWS => {
                self.windows.push((key, None));
                self.windows.len() - 1
            }
            None => {
                self.counts.correlation_invalid += 1;
                return Err(io::ErrorKind::InvalidData.into());
            }
        };
        if closing {
            self.windows[index].1 = Some(at);
        }
        Ok(index as u64 + 1)
    }

    fn frame(&mut self, frame: FrameTiming) -> io::Result<Record> {
        let (start, end) = self.span(frame.draw_start, frame.draw_end)?;
        let dirty = frame.dirty_at.map(|at| self.point(at)).transpose()?;
        if dirty.is_some_and(|at| at > start) {
            self.counts.invalid_clock += 1;
            return Err(io::ErrorKind::InvalidData.into());
        }
        let key = frame.window_id.as_u64();
        Ok(Record {
            kind: 2,
            window: Some(key),
            start: Some(start),
            end: Some(end),
            dirty,
            invalidations: Some(frame.invalidations),
            ..Record::default()
        })
    }

    fn submission(
        &mut self,
        timing: PresentTiming,
        frame: Option<FrameTiming>,
    ) -> io::Result<Record> {
        let (start, end) = self.span(timing.present_start, timing.present_end)?;
        let key = timing.window_id.as_u64();
        let mut record = Record {
            kind: 3,
            window: Some(key),
            start: Some(start),
            end: Some(end),
            ..Record::default()
        };
        if let Some(frame) = frame {
            if frame.window_id != timing.window_id {
                self.counts.correlation_invalid += 1;
                return Err(io::ErrorKind::InvalidData.into());
            }
            let pair = self.frame(frame)?;
            if pair.end.is_some_and(|at| at > start) {
                self.counts.invalid_clock += 1;
                return Err(io::ErrorKind::InvalidData.into());
            }
            // Association only. Never count this embedded FrameTiming as another Draw.
            record.pair_start = pair.start;
            record.pair_end = pair.end;
            record.dirty = pair.dirty;
            record.invalidations = pair.invalidations;
        }
        Ok(record)
    }

    fn source(&mut self, entry: ForegroundJournalEntry) -> io::Result<Option<Record>> {
        use ForegroundJournalEntry as J;
        let record = match entry {
            J::Event(ForegroundEvent::Input(timing)) => {
                let (start, end) = self.span(timing.start, timing.end)?;
                self.counts.input_unassociated += 1;
                Record {
                    kind: 0,
                    start: Some(start),
                    end: Some(end),
                    category: Some(input_category(timing.kind)),
                    caused_invalidation: Some(timing.caused_invalidation),
                    ..Record::default()
                }
            }
            J::Event(ForegroundEvent::Draw(frame)) => self.frame(frame)?,
            J::Event(ForegroundEvent::Present(timing)) => self.submission(timing, None)?,
            J::Boundary(IntervalBoundary::Presented(frame)) => {
                self.submission(frame.presentation, Some(frame.frame))?
            }
            J::FrameState(FrameStateChange::Pending {
                window_id,
                dirty_at,
            }) => {
                let at = self.point(dirty_at)?;
                let key = window_id.as_u64();
                Record {
                    kind: 1,
                    window: Some(key),
                    start: Some(at),
                    end: Some(at),
                    ..Record::default()
                }
            }
            J::FrameState(FrameStateChange::Closed { window_id, at }) => {
                let at = self.point(at)?;
                let key = window_id.as_u64();
                Record {
                    kind: 4,
                    window: Some(key),
                    start: Some(at),
                    end: Some(at),
                    ..Record::default()
                }
            }
            J::Discontinuity { lost } => {
                // The source exposes no gap timestamp. Censor across this conservative
                // observation-time fence; do not manufacture an original timestamp.
                self.barrier = match ns(self.origin, Instant::now()) {
                    Ok(at) => at,
                    Err(error) => {
                        self.counts.invalid_clock += 1;
                        return Err(error);
                    }
                };
                Record {
                    kind: 5,
                    lost: Some(lost),
                    ..Record::default()
                }
            }
            J::Boundary(IntervalBoundary::Idle { ended_at }) => {
                let at = self.point(ended_at)?;
                Record {
                    kind: 6,
                    start: Some(at),
                    end: Some(at),
                    ..Record::default()
                }
            }
            J::Boundary(IntervalBoundary::PowerTransition { ended_at }) => {
                let at = self.point(ended_at)?;
                self.barrier = at;
                Record {
                    kind: 7,
                    start: Some(at),
                    end: Some(at),
                    ..Record::default()
                }
            }
            _ => {
                self.counts.excluded += 1;
                return Ok(None);
            }
        };
        Ok(Some(record))
    }

    fn local(&mut self, event: Event) -> io::Result<Record> {
        let at = self.point(event.at)?;
        let (kind, category) = match event.kind {
            EventKind::SurfaceChanged(surface) => (
                8,
                Some(match surface {
                    app_core::navigation::Surface::Artifact => 0,
                    app_core::navigation::Surface::Compare => 1,
                    app_core::navigation::Surface::Flow => 2,
                }),
            ),
            EventKind::Notify => (9, None),
        };
        Ok(Record {
            kind,
            window: Some(event.window),
            start: Some(at),
            end: Some(at),
            category,
            ..Record::default()
        })
    }

    fn retain(&mut self, mut record: Record, out: &mut impl Write) -> io::Result<()> {
        // One lifetime admission for the whole record, including an optional
        // source-associated draw. Never admit/count the embedded draw twice.
        if let Some(window) = record.window {
            let at = record.end.ok_or(io::ErrorKind::InvalidData)?;
            let Ok(lifetime) = self.window(window, at, record.kind == 4) else {
                return Ok(());
            };
            record.lifetime = Some(lifetime);
        }
        if record.kind == 3 && record.pair_start.is_none() {
            self.counts.unmatched_submission += 1;
        }
        self.counts.eligible += 1;
        if self.counts.retained < RETAINED {
            if self.counts.retained != 0 {
                out.write_all(b",\n")?;
            }
            record.write(out, self.counts.retained + 1)?;
            self.counts.retained += 1;
            self.counts.kinds[record.kind] += 1;
        }
        Ok(())
    }

    fn drain(
        &mut self,
        collector: &mut ForegroundJournalCollector,
        local: &Receiver<Event>,
        out: &mut impl Write,
        deadline: Option<Instant>,
    ) -> io::Result<()> {
        let drained = collector.collect_unseen();
        self.counts.journal_lost += drained.lost;
        let total = drained.entries.len();
        for (index, entry) in drained.entries.into_iter().enumerate() {
            if deadline.is_some_and(|limit| Instant::now() >= limit) {
                self.counts.undrained_journal += (total - index) as u64;
                break;
            }
            self.counts.journal_seen += 1;
            if let Ok(Some(record)) = self.source(entry) {
                self.retain(record, out)?;
            }
        }
        while !deadline.is_some_and(|limit| Instant::now() >= limit) {
            let Ok(event) = local.try_recv() else {
                break;
            };
            self.counts.local_received += 1;
            if let Ok(record) = self.local(event) {
                self.retain(record, out)?;
            }
        }
        Ok(())
    }
}

fn input_category(kind: &str) -> u64 {
    match kind {
        "key_down" => 1,
        "key_up" => 2,
        "modifiers_changed" => 3,
        "mouse_down" => 4,
        "mouse_up" => 5,
        "mouse_move" => 6,
        "mouse_exited" => 7,
        "scroll_wheel" => 8,
        "file_drop" => 9,
        "mouse_pressure" => 10,
        "pinch" => 11,
        "long_press" => 12,
        "touch_drag" => 13,
        "touch" => 14,
        _ => 0, // Unknown source kind stays a closed numeric category, never raw text.
    }
}

fn capture(
    mut collector: ForegroundJournalCollector,
    local: Receiver<Event>,
    origin: Instant,
    control: Arc<Control>,
    report: &Report,
) -> io::Result<()> {
    let profile = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or(io::ErrorKind::NotFound)?);
    let temp = PathBuf::from(std::env::var_os("TEMP").ok_or(io::ErrorKind::NotFound)?);
    let execution = profile
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .ok_or(io::ErrorKind::InvalidInput)?;
    // Isolated profile variables must denote the same components; separator
    // spelling is not part of the identity.
    let env_path = |name: &str| -> io::Result<PathBuf> {
        std::env::var_os(name)
            .map(PathBuf::from)
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    };
    if temp != profile.join("temp")
        || env_path("TMP")? != temp
        || env_path("APPDATA")? != profile.join("roaming")
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let _held = pin_directory(&temp)?;
    marker(&profile.join("run-id"), execution)?;
    let exe = File::open(std::env::current_exe()?)?;
    let (executable, _) = sha256::hash(BufReader::with_capacity(64 * 1024, exe))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(temp.join("r0-s2b.json"))?;
    let mut out = BufWriter::new(LimitedWriter { file, written: 0 });
    let mut minimum = None;
    let mut previous = Instant::now();
    for _ in 0..128 {
        let now = Instant::now();
        let delta = now
            .checked_duration_since(previous)
            .and_then(|d| u64::try_from(d.as_nanos()).ok())
            .ok_or(io::ErrorKind::InvalidData)?;
        if delta > 0 {
            minimum = Some(minimum.map_or(delta, |old: u64| old.min(delta)));
        }
        previous = now;
    }
    let start = ns(origin, Instant::now())?;
    if control.stopped_at_ns().is_some() {
        return Err(io::ErrorKind::Interrupted.into());
    }
    writeln!(
        out,
        "{{\"capture_version\":1,\"execution_id\":\"{execution}\",\"pid\":{},\"executable_sha256\":\"{executable}\",\"gpui_pin\":\"{GPUI_PIN}\",\"feature\":\"r0-s2b-instrumentation\",\"target\":\"{}-pc-windows-msvc\",\"clock\":{{\"domain\":\"native-process-instant-v1\",\"origin_ns\":0,\"unit\":\"ns\",\"resolution\":\"NOT_MEASURED\",\"uncertainty\":\"NOT_MEASURED\",\"mapping\":\"NONE\",\"paired_reads\":128,\"min_positive_delta_ns\":{}}},\"scope_start_ns\":{start},\"records\":[",
        std::process::id(),
        std::env::consts::ARCH,
        Nullable(minimum)
    )?;
    let end = start
        .checked_add(DURATION_NS)
        .ok_or(io::ErrorKind::InvalidData)?;
    let mut observer = Observer {
        origin,
        start,
        end,
        barrier: start,
        windows: Vec::with_capacity(WINDOWS),
        counts: Counts::default(),
    };
    control.start();
    while control.stopped_at_ns().is_none() && ns(origin, Instant::now())? < end {
        observer.drain(&mut collector, &local, &mut out, None)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    control.stop();
    observer.end = end.min(control.stopped_at_ns().ok_or(io::ErrorKind::InvalidData)?);
    let deadline = Instant::now() + DRAIN;
    while control.accounting().2 != 0 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    if control.accounting().2 != 0 {
        return Err(io::ErrorKind::TimedOut.into());
    }
    observer.drain(&mut collector, &local, &mut out, Some(deadline))?;
    let (enqueued, dropped, _) = control.accounting();
    let c = &observer.counts;
    let undrained = enqueued
        .checked_sub(c.local_received)
        .ok_or(io::ErrorKind::InvalidData)?;
    write!(
        out,
        "\n],\"scope_end_ns\":{},\"summary\":{{\"journal_lost\":{},\"journal_seen\":{},\"local_received\":{},\"local_enqueued\":{enqueued},\"local_dropped\":{dropped},\"local_undrained\":{undrained},\"eligible\":{},\"retained\":{},\"retention_dropped\":{},\"excluded\":{},\"pre_scope\":{},\"post_scope\":{},\"invalid_clock\":{},\"barrier_spans\":{},\"correlation_invalid\":{},\"input_unassociated\":{},\"unmatched_submission\":{},\"undrained_journal\":{},\"cancelled\":{},\"source_coverage\":\"OFFERED_FINALIZED_JOURNAL_ONLY\",\"unoffered_loss\":\"UNKNOWN\",\"unfinalized_tail\":\"UNKNOWN\",\"kind_counts\":[{},{},{},{},{},{},{},{},{},{}]}}}}\n",
        observer.end,
        c.journal_lost,
        c.journal_seen,
        c.local_received,
        c.eligible,
        c.retained,
        c.eligible - c.retained,
        c.excluded,
        c.pre_scope,
        c.post_scope,
        c.invalid_clock,
        c.barrier_spans,
        c.correlation_invalid,
        c.input_unassociated,
        c.unmatched_submission,
        c.undrained_journal,
        observer.end < end,
        c.kinds[0],
        c.kinds[1],
        c.kinds[2],
        c.kinds[3],
        c.kinds[4],
        c.kinds[5],
        c.kinds[6],
        c.kinds[7],
        c.kinds[8],
        c.kinds[9]
    )?;
    out.flush()?;
    let mut sink = out.into_inner().map_err(|error| error.into_error())?;
    sink.file.sync_all()?;
    sink.file.rewind()?;
    let (digest, _) = sha256::hash(BufReader::with_capacity(64 * 1024, &mut sink.file))?;
    drop(sink);
    if report.0.load(Ordering::Acquire) != RUNNING {
        return Err(io::ErrorKind::TimedOut.into());
    }
    // The empty directory is the sole visibility commit after file sync/close.
    std::fs::create_dir(temp.join(format!("r0-s2b-committed-{}", &digest[7..])))?;
    report
        .0
        .compare_exchange(RUNNING, COMMITTED, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?;
    Ok(())
}
