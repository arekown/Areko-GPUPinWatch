//! areko-gpupinwatchd: the Areko GPUPinWatch service (see docs/ARCHITEKTUR.md).
//!
//! areko-gpupinwatchd [--probe] [--config PATH] [--state-dir DIR] [--socket PATH] [--seconds N] [--warn A --crit A]
//! Windows also: areko-gpupinwatchd install | uninstall | service
mod assign;
mod guard;
mod ipc_server;
mod nvml;
mod platform;
mod state;

use guard::{log, GpuControl, Guard, NoGpu, Options};
use ipc_server::{Command, Shared};
use gpw_core::config::Config;
use gpw_core::ipc::Reply;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const USAGE: &str = "areko-gpupinwatchd [--probe] [--config PATH] [--state-dir DIR] [--socket PATH] [--seconds N] [--warn A --crit A]
  --probe      never set a power limit, never shut down; only log what would happen
  --warn/--crit  artificial thresholds in A (only together with --probe)";

#[derive(Default)]
struct Args {
    probe: bool,
    config: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    socket: Option<String>,
    seconds: Option<u64>,
    warn: Option<f32>,
    crit: Option<f32>,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    while let Some(k) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{k}: value missing"));
        let amps = |v: String| match v.parse::<f32>() {
            Ok(x) if x.is_finite() && x > 0.0 => Ok(x),
            _ => Err(format!("{k}: not a current in A: {v}")),
        };
        match k.as_str() {
            "--probe" => a.probe = true,
            "--config" => a.config = Some(val()?.into()),
            "--state-dir" => a.state_dir = Some(val()?.into()),
            "--socket" => a.socket = Some(val()?),
            "--seconds" => a.seconds = Some(val()?.parse().map_err(|_| format!("{k}: not a number"))?),
            "--warn" => a.warn = Some(amps(val()?)?),
            "--crit" => a.crit = Some(amps(val()?)?),
            "-h" | "--help" => return Err(String::new()),
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    if (a.warn.is_some() || a.crit.is_some()) && !a.probe {
        return Err("--warn/--crit are only allowed with --probe".into());
    }
    if let (Some(w), Some(c)) = (a.warn, a.crit) {
        if c < w {
            return Err("--crit must not be below --warn".into());
        }
    }
    Ok(a)
}

fn load_config(path: &PathBuf) -> Config {
    match std::fs::read_to_string(path) {
        Ok(s) => Config::from_toml(&s).unwrap_or_else(|e| {
            log(&format!("{}: {e} – Standardwerte", path.display()));
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

fn interval(cfg: &Config) -> Duration {
    Duration::from_millis(1000 / cfg.poll_hz.max(1) as u64)
}

fn run(args: Args, stop: Arc<AtomicBool>) -> Result<(), String> {
    let paths = platform::default_paths();
    let config_path = args.config.unwrap_or(paths.config);
    let state_dir = args.state_dir.unwrap_or(paths.state_dir);
    let endpoint = args.socket.unwrap_or(paths.endpoint);
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("{}: {e}", state_dir.display()))?;
    let cfg = load_config(&config_path);
    if args.probe {
        log("Probe-Modus: setzt keine Grenze, fährt nicht herunter");
    }
    let control: Box<dyn GpuControl> = match nvml::NvmlControl::new() {
        Ok(c) => Box::new(c),
        Err(e) => {
            log(&format!("{e} – überwache ohne Drosseln"));
            Box::new(NoGpu(e))
        }
    };
    let opts = Options { probe: args.probe, state_dir, config_path, warn_a: args.warn, crit_a: args.crit };
    let mut guard =
        Guard::new(cfg, opts, control, Box::new(gpw_sources::discover), Box::new(platform::shutdown));
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let shared = Shared::new(cmd_tx);
    shared.publish(guard.snapshot());
    ipc_server::start(&endpoint, shared.clone())?;
    log(&format!("läuft · {} Hz · IPC {endpoint}", guard.config().poll_hz));

    let start = Instant::now();
    let end = args.seconds.map(|s| start + Duration::from_secs(s));
    let mut next = start;
    while !stop.load(Ordering::SeqCst) && end.is_none_or(|e| Instant::now() < e) {
        guard.tick(start.elapsed().as_millis() as u64);
        shared.publish(guard.snapshot());
        if guard.shutdown_pending() {
            // Give subscribed apps the alarm snapshot first (at most 1 s).
            std::thread::sleep(Duration::from_secs(1));
            guard.execute_shutdown();
            shared.publish(guard.snapshot());
        }
        next += interval(guard.config());
        let now = Instant::now();
        if next < now {
            next = now; // fell behind (slow source): no burst of catch-up readings
        }
        // Wait for the next reading, executing app commands meanwhile.
        while let Some(left) = next.checked_duration_since(Instant::now()) {
            match cmd_rx.recv_timeout(left) {
                Ok((cmd, reply)) => {
                    let r = match cmd {
                        Command::Release(pci) => guard.release(&pci),
                        Command::SetConfig(c) => guard.set_config(c),
                    };
                    let _ = reply.send(match r {
                        Ok(()) => Reply::Ok,
                        Err(message) => Reply::Error { message },
                    });
                    shared.publish(guard.snapshot());
                }
                Err(_) => break,
            }
        }
    }
    ipc_server::stop(&endpoint);
    log("beendet");
    Ok(())
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    {
        let r = match argv.first().map(String::as_str) {
            Some("install") => Some(platform::service::install()),
            Some("uninstall") => Some(platform::service::uninstall()),
            Some("service") => Some(platform::service::dispatch(|stop| run(Args::default(), stop))),
            _ => None,
        };
        if let Some(r) = r {
            if let Err(e) = r {
                eprintln!("areko-gpupinwatchd: {e}");
                std::process::exit(1);
            }
            return;
        }
    }
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("areko-gpupinwatchd: {e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args, Arc::new(AtomicBool::new(false))) {
        eprintln!("areko-gpupinwatchd: {e}");
        std::process::exit(1);
    }
}
