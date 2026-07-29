use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;
use nix::poll::{poll, PollFd, PollFlags};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

use letsnote_wheelpad::config::Config;
use letsnote_wheelpad::detector::{CircularDetector, CoordinateTransform};
use letsnote_wheelpad::error::{Error, Result};
use letsnote_wheelpad::evdev::{GrabGuard, InputDevice};
use letsnote_wheelpad::fsm::{Action, Fsm, FsmState};
use letsnote_wheelpad::runtime::LoopExit;
use letsnote_wheelpad::uinput::{UinputTouchpad, UinputWheel};

static STOP: AtomicBool = AtomicBool::new(false);

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

    // 1. Open the physical touchpad.
    let device_path = match args.device.or_else(|| config.device.clone()) {
        Some(p) => p,
        None => InputDevice::find_by_name(&config.device_name_regex)?,
    };
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
    //    through our virtual touchpad. Releasing the grab is handled
    //    by `Drop` on `input.device` (and by the panic-safety cleanup
    //    after the main loop returns).
    let mut input = input.grab()?;
    info!("physical touchpad grabbed (passthrough mode)");

    // 5. Notify systemd we're ready.
    if let Err(e) = sd_notify_ready() {
        warn!("sd_notify Ready failed (acceptable outside systemd): {e}");
    }

    // 6. Build the algorithm and FSM. History capacity is fixed at 20
    //    to match Windows WheelPad exactly (D-021-followup).
    let transform = CoordinateTransform::new(config.scroll.coordinate_y_scale);
    let mut detector =
        CircularDetector::with_geometry(transform, config.scroll.minimum_rotation_radius);
    let mut fsm = Fsm::with_transform(input.center_x, input.center_y, transform);

    // 7. Signal handling.
    install_signal_handlers()?;

    // 8. Main loop.
    let exit = run_event_loop(
        &mut input,
        &mut vtouchpad,
        &mut vwheel,
        &mut fsm,
        &mut detector,
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
    fsm: &mut Fsm,
    detector: &mut CircularDetector,
    config: &Config,
) -> LoopExit {
    let raw_fd = input.device.as_raw_fd();
    loop {
        if STOP.load(Ordering::Relaxed) {
            return LoopExit::RequestedShutdown;
        }
        // SAFETY: raw_fd is owned by `input.device` which outlives the
        // borrow for the iteration; we never read/write through the
        // BorrowedFd ourselves.
        let borrowed = unsafe { BorrowedFd::borrow_raw(raw_fd) };
        let mut fds = [PollFd::new(&borrowed, PollFlags::POLLIN)];
        let _ready = match poll(&mut fds, -1) {
            Ok(n) => n,
            Err(nix::errno::Errno::EINTR) => {
                if STOP.load(Ordering::Relaxed) {
                    return LoopExit::RequestedShutdown;
                }
                continue;
            }
            Err(source) => return LoopExit::PollFailed { source },
        };

        let revents = fds[0].revents().unwrap_or_else(PollFlags::empty);
        if revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
            return LoopExit::InputDisconnected {
                source: std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    format!("poll reported {revents:?}"),
                ),
            };
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
            // Snapshot pre-step state. The forwarding decision uses
            // BOTH pre and post: pre tells us whether positions need
            // stripping (lift batch), post tells us whether to forward
            // at all (suppress during Scrolling).
            let prev_state = fsm.state();
            let frame = match fsm.contact_id() {
                Some(contact) => pf.contacts.for_contact(contact),
                None => pf.contacts.primary(),
            };
            let action = fsm.step(frame, detector, &config.scroll);
            let now_state = fsm.state();

            match action {
                Action::None => {}
                Action::EmitWheelV(t) => {
                    if let Err(source) = vwheel.emit_v(t) {
                        return LoopExit::VirtualWheelFailed { source };
                    }
                    debug!(ticks = t, "emit vertical");
                }
                Action::EmitWheelH(t) => {
                    if let Err(source) = vwheel.emit_h(t) {
                        return LoopExit::VirtualWheelFailed { source };
                    }
                    debug!(ticks = t, "emit horizontal");
                }
            }

            // Passthrough:
            //   post = Scrolling → suppress entirely (cursor frozen).
            //   pre = Scrolling && post != Scrolling → lift batch:
            //          forward but strip position events, so libinput
            //          sees BTN_TOUCH=0 / tracking_id=-1 without a
            //          synthetic jump from the pre-engagement
            //          coordinate.
            //   otherwise → forward verbatim.
            if !matches!(now_state, FsmState::Scrolling { .. }) {
                let strip_positions = matches!(prev_state, FsmState::Scrolling { .. });
                if let Err(source) = vtouchpad.forward(&pf.events, strip_positions) {
                    return LoopExit::VirtualTouchpadFailed { source };
                }
            }
        }
    }
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

fn install_signal_handlers() -> Result<()> {
    use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

    extern "C" fn handler(_sig: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    let action = SigAction::new(
        SigHandler::Handler(handler),
        SaFlags::empty(),
        SigSet::empty(),
    );
    unsafe {
        sigaction(Signal::SIGTERM, &action).map_err(|source| Error::Signal { source })?;
        sigaction(Signal::SIGINT, &action).map_err(|source| Error::Signal { source })?;
    }
    Ok(())
}
