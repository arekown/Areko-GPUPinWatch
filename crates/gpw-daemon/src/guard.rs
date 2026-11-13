//! Measuring loop as a testable struct: reads every pin source, runs `PinGuard` per card,
//! throttles via [`GpuControl`], trips (shutdown or alarm only), keeps events and history,
//! and builds the IPC snapshot. Time is passed in (`now_ms`, monotonic), so tests drive it.
use crate::assign::Assigner;
use crate::state::{self, GpuState, State};
use gpw_core::config::{Config, TripAction};
use gpw_core::ipc::{Binding, CardView, EventKind, Level, LogEvent, Snapshot, SourceView, HISTORY};
use gpw_core::model::{norm_pci, GpuInfo, SourceInfo, SourceKind, PINS};
use gpw_core::policy::{Decision, Phase, PinGuard};
use gpw_sources::{PinSource, Reading};
use std::collections::VecDeque;
use std::path::PathBuf;

/// Events kept for the snapshot.
pub const EVENTS: usize = 200;
/// Consecutive read failures before a source is dropped and searched again.
pub const MAX_FAILS: u32 = 5;
/// MSI connectors are shown as a card only after they once carried more than this (sum of pins).
const MSI_ACTIVE_A: f32 = 0.5;
/// Seconds between searches for lost sources.
const REDISCOVER_S: u64 = 10;

/// GPU access (NVML in the service, a fake in tests).
pub trait GpuControl {
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, String>;
    fn set_limit(&mut self, pci: &str, watts: f32) -> Result<(), String>;
}

/// Used when NVML is not available: watching and warning go on, throttling reports an error.
pub struct NoGpu(pub String);

impl GpuControl for NoGpu {
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, String> {
        Err(self.0.clone())
    }
    fn set_limit(&mut self, _pci: &str, _watts: f32) -> Result<(), String> {
        Err(self.0.clone())
    }
}

pub type Discover = Box<dyn FnMut(&[GpuInfo]) -> Vec<Box<dyn PinSource>>>;
pub type ShutdownFn = Box<dyn FnMut() -> Result<(), String>>;

pub struct Options {
    /// Never set a limit, never shut down, never write state.json: only log "würde …".
    pub probe: bool,
    pub state_dir: PathBuf,
    /// Where SetConfig is saved.
    pub config_path: PathBuf,
    /// Probe only: artificial thresholds (warn A, crit A), applied on top of the config.
    pub warn_a: Option<f32>,
    pub crit_a: Option<f32>,
}

struct Card {
    source: Box<dyn PinSource>,
    guard: PinGuard,
    fails: u32,
    last: Option<Reading>,
    history: VecDeque<f32>,
    trigger: Option<(usize, f32)>,
    binding: Binding,
    gpu: Option<String>,
    /// Ever carried current (MSI connectors without a cable stay hidden).
    active: bool,
}

impl Card {
    fn new(source: Box<dyn PinSource>) -> Self {
        Card {
            source,
            guard: PinGuard::default(),
            fails: 0,
            last: None,
            history: VecDeque::with_capacity(HISTORY),
            trigger: None,
            binding: Binding::Matching,
            gpu: None,
            active: false,
        }
    }
    fn info(&self) -> &SourceInfo {
        self.source.info()
    }
    fn amps(&self) -> Option<[f32; PINS]> {
        self.last.as_ref().map(|r| r.pins.map(|p| p.amps))
    }
}

pub struct Guard {
    base_cfg: Config,
    cfg: Config,
    opts: Options,
    control: Box<dyn GpuControl>,
    discover: Discover,
    shutdown: ShutdownFn,
    cards: Vec<Card>,
    lost: Vec<SourceInfo>,
    gpus: Vec<GpuInfo>,
    gpu_error: Option<String>,
    state: State,
    assign: Assigner,
    events: VecDeque<LogEvent>,
    shutdown_pending: bool,
    last_discover_ms: u64,
}

pub fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// UTC "YYYYMMDD-HHMMSS" for file names (no date crate needed).
fn utc_stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn log(msg: &str) {
    eprintln!("areko-gpupinwatchd: {msg}");
}

impl Guard {
    pub fn new(cfg: Config, opts: Options, control: Box<dyn GpuControl>, discover: Discover, shutdown: ShutdownFn) -> Self {
        let mut g = Guard {
            base_cfg: cfg.clone(),
            cfg,
            opts,
            control,
            discover,
            shutdown,
            cards: Vec::new(),
            lost: Vec::new(),
            gpus: Vec::new(),
            gpu_error: None,
            state: State::default(),
            assign: Assigner::default(),
            events: VecDeque::with_capacity(EVENTS),
            shutdown_pending: false,
            last_discover_ms: 0,
        };
        g.cfg = g.effective(g.base_cfg.clone());
        g.event(Level::Info, None, EventKind::Started);
        g.refresh_gpus();
        for gpu in &g.gpus {
            log(&format!(
                "GPU {} {} · {:?} W (Grenze {:?} W, min {:?}, max {:?}, Standard {:?})",
                gpu.pci, gpu.name, gpu.power_w, gpu.limit_w, gpu.min_limit_w, gpu.max_limit_w, gpu.default_limit_w
            ));
        }
        g.restore_state();
        let found = (g.discover)(&g.gpus);
        for s in found {
            g.add_source(s);
        }
        // A GPU without any pin source is not protected: say so once.
        let unbound = g.cards.iter().any(|c| c.info().gpu_pci.is_none());
        for gpu in g.gpus.clone() {
            let fixed = g.cards.iter().any(|c| c.info().gpu_pci.as_deref().map(norm_pci).as_deref() == Some(gpu.pci.as_str()));
            if !fixed && !unbound {
                g.event(
                    Level::Warn,
                    Some(gpu.pci.clone()),
                    EventKind::Error { message: format!("no pin source for {}", gpu.name) },
                );
            }
        }
        g
    }

    /// Probe thresholds override the (sanitised) config.
    fn effective(&self, mut cfg: Config) -> Config {
        if self.opts.probe {
            if let Some(w) = self.opts.warn_a {
                cfg.stage1.warn_a = w;
            }
            if let Some(c) = self.opts.crit_a {
                cfg.stage2.crit_a = c;
            }
        }
        cfg
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn event(&mut self, level: Level, gpu: Option<String>, event: EventKind) {
        log(&format!("[{level:?}] {}{event:?}", gpu.as_deref().map(|g| format!("{g} ")).unwrap_or_default()));
        if self.events.len() == EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(LogEvent { ts_ms: wall_ms(), level, gpu, event });
    }

    fn refresh_gpus(&mut self) {
        match self.control.gpus() {
            Ok(v) => {
                self.gpus = v;
                self.gpu_error = None;
            }
            Err(e) => {
                self.gpus.clear();
                if self.gpu_error.as_deref() != Some(e.as_str()) {
                    self.event(Level::Warn, None, EventKind::Error { message: e.clone() });
                    self.gpu_error = Some(e);
                }
            }
        }
    }

    fn save_state(&mut self) {
        if self.opts.probe {
            return;
        }
        if let Err(e) = self.state.save(&state::path(&self.opts.state_dir)) {
            self.event(Level::Warn, None, EventKind::Error { message: format!("state.json: {e}") });
        }
    }

    /// Service start: GPUs left throttled by a previous run get their old limit back.
    fn restore_state(&mut self) {
        let file = state::path(&self.opts.state_dir);
        let old = State::load(&file);
        for (pci, gs) in old.gpus {
            let Some(prev) = gs.prev_limit_w else { continue };
            if self.opts.probe {
                log(&format!("Probe: würde {pci} die alte Grenze {prev} W zurückgeben"));
                continue;
            }
            match self.control.set_limit(&pci, prev) {
                Ok(()) => self.event(Level::Info, Some(pci), EventKind::Released { limit_w: prev }),
                Err(e) => self.event(Level::Alarm, Some(pci), EventKind::Error { message: format!("restore limit: {e}") }),
            }
        }
        self.save_state();
    }

    fn add_source(&mut self, s: Box<dyn PinSource>) {
        let info = s.info().clone();
        self.lost.retain(|l| l.id != info.id);
        self.event(Level::Info, info.gpu_pci.clone(), EventKind::SourceFound { kind: info.kind, label: info.label.clone() });
        self.cards.push(Card::new(s));
    }

    /// One reading of every source. `now_ms` is monotonic.
    pub fn tick(&mut self, now_ms: u64) {
        self.refresh_gpus();
        let mut dropped = Vec::new();
        for i in 0..self.cards.len() {
            let card = &mut self.cards[i];
            match card.source.read() {
                Ok(r) => {
                    card.fails = 0;
                    card.last = Some(r);
                }
                Err(e) => {
                    card.fails += 1;
                    card.last = None;
                    if card.fails == 1 {
                        log(&format!("{}: {e}", card.info().label));
                    }
                    if card.fails >= MAX_FAILS {
                        dropped.push(i);
                    }
                }
            }
            let amps = card.amps();
            if let Some(a) = &amps {
                let top = a.iter().copied().fold(0.0, f32::max);
                if card.history.len() == HISTORY {
                    card.history.pop_front();
                }
                card.history.push_back(top);
                if a.iter().sum::<f32>() > MSI_ACTIVE_A {
                    card.active = true;
                }
            }
            let a = self.assign.update(card.source.info(), amps.as_ref(), &self.gpus);
            card.binding = a.binding;
            card.gpu = a.gpu.clone();
            let label = card.info().label.clone();
            let decision = card.guard.step(&self.cfg, now_ms, amps.as_ref());
            if a.new_match {
                let gpu = a.gpu.clone().unwrap_or_default();
                self.event(Level::Info, a.gpu.clone(), EventKind::Matched { label: label.clone(), gpu });
            }
            match decision {
                Decision::Nothing => {}
                Decision::Throttle { pin, amps } => self.throttle(i, pin, amps),
                Decision::Trip { pin, amps } => self.trip(i, pin, amps),
            }
        }
        for i in dropped.into_iter().rev() {
            let card = self.cards.remove(i);
            let info = card.info().clone();
            self.event(Level::Warn, card.gpu.clone(), EventKind::SourceLost { label: info.label.clone() });
            self.lost.push(info);
            self.last_discover_ms = 0; // search right away
        }
        if !self.lost.is_empty() && (self.last_discover_ms == 0 || now_ms - self.last_discover_ms >= REDISCOVER_S * 1000) {
            self.last_discover_ms = now_ms.max(1);
            let found = (self.discover)(&self.gpus);
            for s in found {
                if !self.cards.iter().any(|c| c.info().id == s.info().id) {
                    self.add_source(s);
                }
            }
        }
    }

    fn gpu_info(&self, pci: &str) -> Option<GpuInfo> {
        self.gpus.iter().find(|g| g.pci == pci).cloned()
    }

    /// Stage 1: remember the current limit, lower it.
    fn throttle(&mut self, i: usize, pin: usize, amps: f32) {
        self.cards[i].trigger = Some((pin, amps));
        let label = self.cards[i].info().label.clone();
        let Some(pci) = self.cards[i].gpu.clone() else {
            let message = format!("cannot throttle {label}: GPU not assigned yet");
            self.event(Level::Alarm, None, EventKind::Error { message });
            return;
        };
        let Some(gpu) = self.gpu_info(&pci) else {
            let why = self.gpu_error.clone().unwrap_or_else(|| "GPU not found".into());
            self.event(Level::Alarm, Some(pci), EventKind::Error { message: format!("cannot throttle: {why}") });
            return;
        };
        let target = self.cfg.throttle_watts(gpu.default_limit_w, gpu.min_limit_w, gpu.max_limit_w);
        let gs = self.state.gpus.entry(pci.clone()).or_insert(GpuState { phase: Phase::Ok, prev_limit_w: None });
        if gs.prev_limit_w.is_none() {
            gs.prev_limit_w = gpu.limit_w;
        }
        gs.phase = Phase::Throttled;
        let result = if self.opts.probe {
            log(&format!("Probe: würde Grenze von {pci} von {:?} W auf {target} W setzen", gpu.limit_w));
            Ok(())
        } else {
            self.control.set_limit(&pci, target)
        };
        match result {
            Ok(()) => self.event(Level::Warn, Some(pci), EventKind::Throttled { target_w: target, pin, amps }),
            Err(e) => self.event(Level::Alarm, Some(pci), EventKind::Error { message: format!("throttle failed: {e}") }),
        }
        self.save_state();
    }

    /// Stage 2: event, trip log, then shutdown (after the caller pushed the snapshot) or alarm only.
    fn trip(&mut self, i: usize, pin: usize, amps: f32) {
        self.cards[i].trigger = Some((pin, amps));
        let shutdown = self.cfg.stage2.action == TripAction::Shutdown;
        let gpu = self.cards[i].gpu.clone();
        if let Some(pci) = &gpu {
            let gs = self.state.gpus.entry(pci.clone()).or_insert(GpuState { phase: Phase::Ok, prev_limit_w: None });
            gs.phase = Phase::Alarm;
            self.save_state();
        }
        self.event(Level::Alarm, gpu, EventKind::Alarm { pin, amps, shutdown });
        self.write_trip_log(i, pin, amps);
        if shutdown {
            self.shutdown_pending = true;
        }
    }

    fn write_trip_log(&mut self, i: usize, pin: usize, amps: f32) {
        let now = wall_ms();
        let mut s = format!(
            "Areko GPUPinWatch {} trip {} (UTC)\nsource: {} ({})\npin {}: {amps:.2} A\naction: {:?}\n\n",
            gpw_core::VERSION,
            utc_stamp(now),
            self.cards[i].info().label,
            self.cards[i].info().id,
            pin + 1,
            self.cfg.stage2.action
        );
        for c in &self.cards {
            let gpu = c.gpu.as_deref().and_then(|p| self.gpu_info(p));
            s += &format!(
                "{} [{}] gpu={:?} power={:?} W limit={:?} W phase={:?}\n  amps={:?}\n  last max per sample={:?}\n",
                c.info().label,
                c.info().id,
                c.gpu,
                gpu.as_ref().and_then(|g| g.power_w),
                gpu.as_ref().and_then(|g| g.limit_w),
                c.guard.phase(),
                c.amps(),
                c.history.iter().rev().take(20).rev().collect::<Vec<_>>()
            );
        }
        s += &format!("\nconfig:\n{}", self.cfg.to_toml());
        let file = self.opts.state_dir.join(format!("trip-{}.log", utc_stamp(now)));
        if let Err(e) = std::fs::write(&file, s) {
            self.event(Level::Warn, None, EventKind::Error { message: format!("{}: {e}", file.display()) });
        } else {
            log(&format!("Protokoll {}", file.display()));
        }
    }

    pub fn shutdown_pending(&self) -> bool {
        self.shutdown_pending
    }

    /// Runs the pending stage-2 shutdown (call after the snapshot was pushed).
    pub fn execute_shutdown(&mut self) {
        if !std::mem::take(&mut self.shutdown_pending) {
            return;
        }
        if self.opts.probe {
            log("Probe: würde jetzt herunterfahren");
            return;
        }
        log("fahre herunter");
        if let Err(e) = (self.shutdown)() {
            self.event(Level::Alarm, None, EventKind::Error { message: format!("shutdown failed: {e}") });
        }
    }

    /// Gives a throttled GPU (or an unbound source in alarm state, by source id) its previous limit back.
    pub fn release(&mut self, key: &str) -> Result<(), String> {
        let key = key.trim();
        let pci = norm_pci(key);
        let hit: Vec<usize> = (0..self.cards.len())
            .filter(|&i| self.cards[i].gpu.as_deref() == Some(pci.as_str()) || self.cards[i].info().id == key)
            .collect();
        let gs = self.state.gpus.get(&pci).cloned();
        if hit.iter().all(|&i| self.cards[i].guard.phase() == Phase::Ok) && gs.is_none() {
            return Err(format!("nothing to release for {key}"));
        }
        let prev = gs.as_ref().and_then(|g| g.prev_limit_w);
        if let Some(prev) = prev {
            if self.opts.probe {
                log(&format!("Probe: würde {pci} die alte Grenze {prev} W zurückgeben"));
            } else if let Err(e) = self.control.set_limit(&pci, prev) {
                self.event(Level::Alarm, Some(pci), EventKind::Error { message: format!("release failed: {e}") });
                return Err(e);
            }
        }
        for &i in &hit {
            self.cards[i].guard.release();
            self.cards[i].trigger = None;
        }
        self.state.gpus.remove(&pci);
        self.save_state();
        let limit_w = prev.or_else(|| self.gpu_info(&pci).and_then(|g| g.limit_w)).unwrap_or(0.0);
        let gpu = if gs.is_some() || hit.iter().any(|&i| self.cards[i].gpu.is_some()) { Some(pci) } else { None };
        self.event(Level::Info, gpu, EventKind::Released { limit_w });
        Ok(())
    }

    /// New settings from the app: sanitise, save, apply.
    pub fn set_config(&mut self, cfg: Config) -> Result<(), String> {
        let cfg = cfg.sanitize();
        if let Some(dir) = self.opts.config_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&self.opts.config_path, cfg.to_toml()).map_err(|e| format!("{}: {e}", self.opts.config_path.display()))?;
        self.cfg = self.effective(cfg.clone());
        self.base_cfg = cfg;
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        let cards = self
            .cards
            .iter()
            .filter(|c| c.info().kind != SourceKind::MsiPsu || c.active)
            .map(|c| {
                let gpu = c.gpu.as_deref().and_then(|p| self.gpu_info(p));
                let volts = c.last.as_ref().and_then(|r| {
                    let v: Vec<f32> = r.pins.iter().filter_map(|p| p.volts).collect();
                    <[f32; PINS]>::try_from(v).ok()
                });
                CardView {
                    source_id: c.info().id.clone(),
                    source_kind: c.info().kind,
                    source_label: c.info().label.clone(),
                    binding: c.binding,
                    gpu_pci: c.gpu.clone(),
                    gpu_name: gpu.as_ref().map(|g| g.name.clone()),
                    amps: c.amps(),
                    volts,
                    power_w: gpu.as_ref().and_then(|g| g.power_w),
                    limit_w: gpu.as_ref().and_then(|g| g.limit_w),
                    prev_limit_w: c.gpu.as_ref().and_then(|p| self.state.gpus.get(p)).and_then(|g| g.prev_limit_w),
                    phase: c.guard.phase(),
                    trigger: c.trigger,
                    history_max: c.history.iter().copied().collect(),
                }
            })
            .collect();
        let mut sources: Vec<SourceView> = self
            .cards
            .iter()
            .map(|c| SourceView {
                id: c.info().id.clone(),
                kind: c.info().kind,
                label: c.info().label.clone(),
                detail: c.info().detail.clone(),
                ok: c.last.is_some(),
            })
            .collect();
        sources.extend(self.lost.iter().map(|l| SourceView {
            id: l.id.clone(),
            kind: l.kind,
            label: l.label.clone(),
            detail: l.detail.clone(),
            ok: false,
        }));
        Snapshot {
            version: gpw_core::VERSION.to_string(),
            poll_hz: self.cfg.poll_hz,
            age_ms: 0,
            cards,
            sources,
            events: self.events.iter().cloned().collect(),
            config: self.cfg.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpw_core::model::Pin;
    use gpw_sources::SourceError;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    const PCI: &str = "0000:02:00.0";

    /// Fake NVML: one GPU, records every limit it is asked to set.
    struct TestGpu {
        limit: f32,
        set: Rc<RefCell<Vec<f32>>>,
    }
    impl GpuControl for TestGpu {
        fn gpus(&mut self) -> Result<Vec<GpuInfo>, String> {
            Ok(vec![GpuInfo {
                pci: PCI.into(),
                name: "RTX 5090".into(),
                power_w: Some(550.0),
                limit_w: Some(self.limit),
                min_limit_w: Some(400.0),
                max_limit_w: Some(600.0),
                default_limit_w: Some(600.0),
            }])
        }
        fn set_limit(&mut self, pci: &str, watts: f32) -> Result<(), String> {
            assert_eq!(pci, PCI);
            self.limit = watts;
            self.set.borrow_mut().push(watts);
            Ok(())
        }
    }

    /// Fake on-card sensor: pin 3 carries whatever `top` holds.
    struct TestSource {
        info: SourceInfo,
        top: Arc<Mutex<f32>>,
    }
    impl PinSource for TestSource {
        fn info(&self) -> &SourceInfo {
            &self.info
        }
        fn read(&mut self) -> Result<Reading, SourceError> {
            let mut pins = [Pin { amps: 5.0, volts: Some(12.1) }; PINS];
            pins[3].amps = *self.top.lock().unwrap();
            Ok(Reading { pins })
        }
    }

    fn run(probe: bool) -> (Vec<f32>, u32, Snapshot) {
        let dir = std::env::temp_dir().join(format!("gpw-guard-test-{}-{probe}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let set = Rc::new(RefCell::new(Vec::new()));
        let shutdowns = Rc::new(Cell::new(0u32));
        let top = Arc::new(Mutex::new(5.0f32));
        let src_top = top.clone();
        let sd = shutdowns.clone();
        let opts = Options {
            probe,
            state_dir: dir.clone(),
            config_path: dir.join("config.toml"),
            warn_a: probe.then_some(6.0),
            crit_a: probe.then_some(7.0),
        };
        let mut g = Guard::new(
            Config::default(), // 8.5 A / 1 s, settle 2 s, 9.2 A / 1.5 s, shutdown
            opts,
            Box::new(TestGpu { limit: 600.0, set: set.clone() }),
            Box::new(move |_| {
                let info = SourceInfo {
                    id: format!("astral:{PCI}"),
                    kind: SourceKind::AstralChip,
                    label: "ROG Astral RTX 5090".into(),
                    detail: "test".into(),
                    gpu_pci: Some(PCI.into()),
                };
                vec![Box::new(TestSource { info, top: src_top.clone() }) as Box<dyn PinSource>]
            }),
            Box::new(move || {
                sd.set(sd.get() + 1);
                Ok(())
            }),
        );
        let mut t = 0;
        let mut tick = |g: &mut Guard, a: f32| {
            *top.lock().unwrap() = a;
            g.tick(t);
            t += 500;
        };
        // Probe thresholds are 6 / 7 A; real ones 8.5 / 9.2 A. 9.5 A exceeds both.
        tick(&mut g, 5.0);
        assert_eq!(g.snapshot().cards[0].phase, Phase::Ok);
        for _ in 0..3 {
            tick(&mut g, 9.5); // 0.5 .. 1.5 s: throttle after 1 s hold
        }
        let snap = g.snapshot();
        assert_eq!(snap.cards[0].phase, Phase::Throttled);
        assert_eq!(snap.cards[0].binding, Binding::Fixed);
        assert_eq!(snap.cards[0].prev_limit_w, Some(600.0));
        assert!(!g.shutdown_pending());
        for _ in 0..8 {
            tick(&mut g, 9.5); // settle 2 s + hold 1.5 s
        }
        assert_eq!(g.snapshot().cards[0].phase, Phase::Alarm);
        assert!(g.shutdown_pending());
        g.execute_shutdown();
        assert!(std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("trip-")));
        g.release(PCI).unwrap();
        let snap = g.snapshot();
        assert_eq!(snap.cards[0].phase, Phase::Ok);
        assert_eq!(snap.cards[0].prev_limit_w, None);
        let kinds: Vec<&EventKind> = snap.events.iter().map(|e| &e.event).collect();
        assert!(kinds.iter().any(|k| matches!(k, EventKind::Throttled { target_w, pin: 3, .. } if *target_w == 400.0)));
        assert!(kinds.iter().any(|k| matches!(k, EventKind::Alarm { pin: 3, shutdown: true, .. })));
        assert!(kinds.iter().any(|k| matches!(k, EventKind::Released { limit_w } if *limit_w == 600.0)));
        let _ = std::fs::remove_dir_all(&dir);
        let set = set.borrow().clone();
        (set, shutdowns.get(), snap)
    }

    #[test]
    fn throttle_trip_release_real_and_probe() {
        // Real: limit 600 -> 400 W, shutdown once, release gives 600 W back.
        let (set, shutdowns, _) = run(false);
        assert_eq!(set, vec![400.0, 600.0]);
        assert_eq!(shutdowns, 1);
        // Probe (artificially low thresholds): same stages, but nothing is set or shut down.
        let (set, shutdowns, snap) = run(true);
        assert!(set.is_empty());
        assert_eq!(shutdowns, 0);
        assert_eq!(snap.config.stage1.warn_a, 6.0);
    }
}
