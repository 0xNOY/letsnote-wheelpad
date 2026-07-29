use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::path::PathBuf;

use clap::Parser;
use nix::poll::{poll, PollFd, PollFlags};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

use letsnote_wheelpad::config::Config;
use letsnote_wheelpad::detector::CoordinateTransform;
use letsnote_wheelpad::error::{Error, Result};
use letsnote_wheelpad::evdev::{GrabGuard, InputDevice, PhysicalFrame};
use letsnote_wheelpad::fsm::Action;
use letsnote_wheelpad::proxy::FrameProcessor;
use letsnote_wheelpad::runtime::{InstanceLock, LoopExit, ShutdownSignal};
use letsnote_wheelpad::uinput::{UinputTouchpad, UinputWheel};

#[derive(Parser, Debug)]
#[command(
    name = "letsnote-wheelpad",
    version,
    about = "Userland daemon: Panasonic Let's Note WheelPad circular touchpad scroll on Linux"
)]
struct Args {
    /// Path to config file. Defaults to $XDG_CONFIG_HOME/letsnote-wheelpad/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Override the evdev device path (e.g. /dev/input/event4). Bypasses
    /// the device_name_regex search.
    #[arg(long)]
    device: Option<PathBuf>,

    /// Increase logging verbosity to DEBUG.
    #[arg(long)]
    debug: bool,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("letsnote-wheelpad: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let config_path = args.config.unwrap_or_else(Config::default_path);
    let config = Config::load(&config_path)?;

    init_tracing(&config, args.debug);

    info!(?config_path, "config loaded");
    debug!(?config, "effective config");

    // Block shutdown signals before any resource is grabbed. signalfd
    // makes signal delivery part of the same poll set as evdev, closing
    // the check-then-sleep lost-wakeup race.
    let mut shutdown = ShutdownSignal::new()?;

    // 1. Open the physical touchpad.
    let device_path = match args.device.or_else(|| config.device.clone()) {
        Some(p) => p,
        None => InputDevice::find_by_name(&config.device_name_regex)?,
    };
    let instance_lock = InstanceLock::acquire(&device_path)?;
    debug!(path = %instance_lock.path().display(), "instance lock acquired");
    info!(path = %device_path.display(), "opening touchpad");
    let input = InputDevice::open(&device_path)?;
    info!(
        x_range = %format!("[{}, {}]", input.abs_x_min, input.abs_x_max),
        y_range = %format!("[{}, {}]", input.abs_y_min, input.abs_y_max),
        center = %format!("({}, {})", input.center_x, input.center_y),
        "touchpad ranges queried"
    );

    // 2. Construct the virtual touchpad BEFORE we grab the physical
    //    pad — it has to read the physical pad's capabilities, and we
    //    want any uinput-creation failure to happen before libinput
    //    loses access. If uinput device creation fails (e.g., missing
    //    kernel module) we exit cleanly without a grab held.
    let mut vtouchpad = UinputTouchpad::create_from_physical(&input.device)?;
    info!("virtual touchpad created");

    // 3. Construct the virtual wheel — same lifecycle considerations.
    let mut vwheel = UinputWheel::create()?;
    info!("virtual wheel created");

    // 4. Grab the physical pad permanently. After this point libinput
    //    sees no events from the physical device; everything flows
    //    through our virtual touchpad. GrabGuard releases the grab on
    //    every return path and during unwind.
    let mut input = input.grab()?;
    info!("physical touchpad grabbed (passthrough mode)");

    // 5. Notify systemd we're ready.
    if let Err(e) = sd_notify_ready() {
        warn!("sd_notify Ready failed (acceptable outside systemd): {e}");
    }

    // 6. Build the algorithm and FSM. History capacity is fixed at 20
    //    to match Windows WheelPad exactly (D-021-followup).
    let transform = CoordinateTransform::new(config.scroll.coordinate_y_scale);
    let mut processor = FrameProcessor::new(
        input.center_x,
        input.center_y,
        transform,
        config.scroll.minimum_rotation_radius,
        &input.snapshot(),
    );

    // 7. Main loop.
    let exit = run_event_loop(
        &mut input,
        &mut vtouchpad,
        &mut vwheel,
        &mut processor,
        &mut shutdown,
        &config,
    );
    info!(reason = %exit, "input loop stopped");
    if exit.is_success() {
        Ok(())
    } else {
        Err(Error::Runtime { source: exit })
    }
}

fn run_event_loop(
    input: &mut GrabGuard,
    vtouchpad: &mut UinputTouchpad,
    vwheel: &mut UinputWheel,
    processor: &mut FrameProcessor,
    shutdown: &mut ShutdownSignal,
    config: &Config,
) -> LoopExit {
    let raw_fd = input.device.as_raw_fd();
    let mut routed_events = Vec::new();
    loop {
        let (input_revents, signal_revents) = {
            // SAFETY: raw_fd is owned by `input.device` which outlives
            // this poll call; no access occurs through the borrowed FD.
            let borrowed = unsafe { BorrowedFd::borrow_raw(raw_fd) };
            let signal_fd = shutdown.fd().as_fd();
            let mut fds = [
                PollFd::new(&borrowed, PollFlags::POLLIN),
                PollFd::new(&signal_fd, PollFlags::POLLIN),
            ];
            let timeout_ms = if processor.is_scrolling() { 1_000 } else { -1 };
            match poll(&mut fds, timeout_ms) {
                Ok(0) => {
                    let frame = match input.reconcile_liveness_frame() {
                        Ok(frame) => frame,
                        Err(source) => return LoopExit::InputReadFailed { source },
                    };
                    if let Some(exit) = process_and_emit(
                        &frame,
                        processor,
                        &config.scroll,
                        vtouchpad,
                        vwheel,
                        &mut routed_events,
                    ) {
                        return exit;
                    }
                    continue;
                }
                Ok(_) => {}
                Err(nix::errno::Errno::EINTR) => continue,
                Err(source) => return LoopExit::PollFailed { source },
            }
            (
                fds[0].revents().unwrap_or_else(PollFlags::empty),
                fds[1].revents().unwrap_or_else(PollFlags::empty),
            )
        };

        if signal_revents.contains(PollFlags::POLLIN) {
            match shutdown.read_requested() {
                Ok(true) => return LoopExit::RequestedShutdown,
                Ok(false) => {}
                Err(source) => return LoopExit::SignalReadFailed { source },
            }
        }
        if signal_revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL)
        {
            return LoopExit::SignalReadFailed {
                source: nix::errno::Errno::EIO,
            };
        }

        if input_revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
            return LoopExit::InputDisconnected {
                source: std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    format!("poll reported {input_revents:?}"),
                ),
            };
        }
        if !input_revents.contains(PollFlags::POLLIN) {
            continue;
        }

        let frames = match input.poll_frames() {
            Ok(fs) => fs,
            Err(source) if source.raw_os_error() == Some(libc::ENODEV) => {
                return LoopExit::InputDisconnected { source };
            }
            Err(source) => return LoopExit::InputReadFailed { source },
        };

        if frames.is_empty() {
            // No SYN_REPORT in this fetch (events were only partial).
            continue;
        }

        for pf in frames {
            if let Some(exit) = process_and_emit(
                &pf,
                processor,
                &config.scroll,
                vtouchpad,
                vwheel,
                &mut routed_events,
            ) {
                return exit;
            }
        }
    }
}

fn process_and_emit(
    frame: &PhysicalFrame,
    processor: &mut FrameProcessor,
    scroll: &letsnote_wheelpad::config::Scroll,
    vtouchpad: &mut UinputTouchpad,
    vwheel: &mut UinputWheel,
    routed_events: &mut Vec<evdev::InputEvent>,
) -> Option<LoopExit> {
    let action = match processor.process_frame(frame, scroll, routed_events) {
        Ok(action) => action,
        Err(source) => return Some(LoopExit::InputReadFailed { source }),
    };
    match action {
        Action::None => {}
        Action::EmitWheelV(ticks) => {
            if let Err(source) = vwheel.emit_v(ticks) {
                return Some(LoopExit::VirtualWheelFailed { source });
            }
            debug!(ticks, "emit vertical");
        }
        Action::EmitWheelH(ticks) => {
            if let Err(source) = vwheel.emit_h(ticks) {
                return Some(LoopExit::VirtualWheelFailed { source });
            }
            debug!(ticks, "emit horizontal");
        }
    }
    if let Err(source) = vtouchpad.forward(routed_events) {
        return Some(LoopExit::VirtualTouchpadFailed { source });
    }
    None
}

fn init_tracing(config: &Config, debug_flag: bool) {
    let level = if debug_flag {
        "debug".to_string()
    } else {
        config.log.level.clone()
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("letsnote_wheelpad={level}")));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn sd_notify_ready() -> std::io::Result<()> {
    use libsystemd::daemon::{notify, NotifyState};
    notify(false, &[NotifyState::Ready])
        .map(|_| ())
        .map_err(|e| std::io::Error::other(e.to_string()))
}
