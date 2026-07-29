// Virtual devices created via /dev/uinput.
//
// We expose TWO virtual devices:
//
// 1. UinputWheel — virtual mouse wheel that carries REL_WHEEL{,_HI_RES}
//    and REL_HWHEEL{,_HI_RES}. This is where the scroll ticks come out.
//
// 2. UinputTouchpad — virtual touchpad that mirrors the physical pad's
//    capabilities. We grab the physical pad permanently at startup and
//    forward its events through this device, suppressing position
//    updates while the FSM is in Scrolling state. Libinput attaches to
//    the virtual pad only, so it can never have stale state from a
//    transient grab/ungrab cycle.

use std::io;
use std::path::Path;

use evdev::{
    raw_stream::RawDevice,
    uinput::{VirtualDevice, VirtualDeviceBuilder},
    AbsInfo, AttributeSet, BusType, EventType, InputEvent, InputId, PropType, RelativeAxisType,
    UinputAbsSetup,
};
use tracing::warn;

use crate::error::{Error, Result};

// --- Wheel device --------------------------------------------------------

const WHEEL_NAME: &str = "Let's Note WheelPad (virtual wheel)";
pub(crate) const VENDOR_ID: u16 = 0x6c6e; // ASCII "ln"
pub(crate) const WHEEL_PRODUCT_ID: u16 = 0x7770; // ASCII "wp"
const VERSION: u16 = 1;

/// One wheel "tick" = 120 hi-res units; same value real mice emit. Keeps
/// libinput's smooth-scroll math happy on Wayland and X11 alike.
const HI_RES_STEP: i32 = 120;

pub struct UinputWheel {
    dev: VirtualDevice,
}

impl UinputWheel {
    pub fn create() -> Result<Self> {
        if !Path::new("/dev/uinput").exists() {
            return Err(Error::UinputMissing);
        }

        let mut rel = AttributeSet::<RelativeAxisType>::new();
        rel.insert(RelativeAxisType::REL_WHEEL);
        rel.insert(RelativeAxisType::REL_WHEEL_HI_RES);
        rel.insert(RelativeAxisType::REL_HWHEEL);
        rel.insert(RelativeAxisType::REL_HWHEEL_HI_RES);

        let dev = VirtualDeviceBuilder::new()
            .map_err(|source| Error::UinputCreate { source })?
            .name(WHEEL_NAME)
            .input_id(InputId::new(
                BusType::BUS_VIRTUAL,
                VENDOR_ID,
                WHEEL_PRODUCT_ID,
                VERSION,
            ))
            .with_relative_axes(&rel)
            .map_err(|source| Error::UinputCreate { source })?
            .build()
            .map_err(|source| Error::UinputCreate { source })?;

        Ok(Self { dev })
    }

    /// Emit `ticks` vertical wheel notches. Positive = scroll up
    /// (libinput convention). The caller already applied
    /// `reverse_vertical` flipping; this function does not interpret
    /// signs further.
    pub fn emit_v(&mut self, ticks: i32) -> io::Result<()> {
        self.emit_axis(
            ticks,
            RelativeAxisType::REL_WHEEL,
            RelativeAxisType::REL_WHEEL_HI_RES,
        )
    }

    /// Emit `ticks` horizontal wheel notches. Positive = scroll right.
    pub fn emit_h(&mut self, ticks: i32) -> io::Result<()> {
        self.emit_axis(
            ticks,
            RelativeAxisType::REL_HWHEEL,
            RelativeAxisType::REL_HWHEEL_HI_RES,
        )
    }

    fn emit_axis(
        &mut self,
        ticks: i32,
        axis: RelativeAxisType,
        axis_hi_res: RelativeAxisType,
    ) -> io::Result<()> {
        if ticks == 0 {
            return Ok(());
        }
        let events = [
            InputEvent::new(EventType::RELATIVE, axis_hi_res.0, ticks * HI_RES_STEP),
            InputEvent::new(EventType::RELATIVE, axis.0, ticks),
        ];
        self.dev.emit(&events)
    }
}

// --- Touchpad passthrough device -----------------------------------------

pub(crate) const TOUCHPAD_PRODUCT_ID: u16 = 0x7470; // ASCII "tp"

pub struct UinputTouchpad {
    dev: VirtualDevice,
}

impl UinputTouchpad {
    /// Construct a virtual touchpad that mirrors `physical`'s
    /// capabilities. The virtual device borrows the physical pad's
    /// name (with a suffix) so libinput's name-regex quirks keep
    /// matching, but uses a distinct vendor/product so libinput
    /// classifies it as a separate device.
    pub fn create_from_physical(physical: &RawDevice) -> Result<Self> {
        if !Path::new("/dev/uinput").exists() {
            return Err(Error::UinputMissing);
        }

        let name = format!(
            "{} (letsnote-wheelpad)",
            physical.name().unwrap_or("Touchpad")
        );

        let mut builder = VirtualDeviceBuilder::new()
            .map_err(|source| Error::UinputCreate { source })?
            .name(&name)
            .input_id(InputId::new(
                BusType::BUS_VIRTUAL,
                VENDOR_ID,
                TOUCHPAD_PRODUCT_ID,
                VERSION,
            ));

        // Mirror EV_KEY codes (BTN_TOUCH, BTN_TOOL_FINGER, BTN_LEFT, …).
        if let Some(keys) = physical.supported_keys() {
            builder = builder
                .with_keys(keys)
                .map_err(|source| Error::UinputCreate { source })?;
        }

        // Mirror EV_REL (rare on touchpads, but cheap to copy).
        if let Some(rel) = physical.supported_relative_axes() {
            builder = builder
                .with_relative_axes(rel)
                .map_err(|source| Error::UinputCreate { source })?;
        }

        // Mirror EV_ABS. Each axis carries its own (min, max, fuzz, flat,
        // resolution); we copy those verbatim so libinput's coordinate
        // interpretation matches. `get_abs_state` returns the kernel
        // `input_absinfo` struct, which we wrap in evdev's safe
        // `AbsInfo` constructor.
        if let Some(axes) = physical.supported_absolute_axes() {
            let abs_state = physical
                .get_abs_state()
                .map_err(|source| Error::EvdevRead { source })?;
            for axis in axes.iter() {
                let raw = abs_state[axis.0 as usize];
                let info = AbsInfo::new(
                    raw.value,
                    raw.minimum,
                    raw.maximum,
                    raw.fuzz,
                    raw.flat,
                    raw.resolution,
                );
                let setup = UinputAbsSetup::new(axis, info);
                builder = builder
                    .with_absolute_axis(&setup)
                    .map_err(|source| Error::UinputCreate { source })?;
            }
        }

        // Mirror EV_MSC (MSC_TIMESTAMP, MSC_SERIAL, …).
        if let Some(msc) = physical.misc_properties() {
            builder = builder
                .with_msc(msc)
                .map_err(|source| Error::UinputCreate { source })?;
        }

        // Mirror INPUT_PROP_* properties EXCEPT INPUT_PROP_SEMI_MT.
        // INPUT_PROP_POINTER and INPUT_PROP_BUTTONPAD are essential
        // for libinput to classify the virtual device as a touchpad
        // and a clickpad respectively — we want to keep those.
        // SEMI_MT advertises legacy semi-multi-touch, which makes
        // libinput downgrade multi-finger gesture handling; even if
        // the physical pad reports it (some older Synaptics firmware
        // does), our virtual pad delivers the events libinput's
        // full-MT path expects, so SEMI_MT only confuses things.
        let mut filtered_props = AttributeSet::<PropType>::new();
        for prop in physical.properties().iter() {
            if prop == PropType::SEMI_MT {
                warn!(
                    "filtering INPUT_PROP_SEMI_MT from virtual touchpad \
                     (would degrade libinput multi-finger handling)"
                );
                continue;
            }
            filtered_props.insert(prop);
        }
        builder = builder
            .with_properties(&filtered_props)
            .map_err(|source| Error::UinputCreate { source })?;

        let dev = builder
            .build()
            .map_err(|source| Error::UinputCreate { source })?;

        Ok(Self { dev })
    }

    /// Emit one frame after event-level routing. `events` must not
    /// contain SYN_REPORT because evdev's `VirtualDevice::emit()`
    /// appends exactly one report terminator.
    pub fn forward(&mut self, events: &[InputEvent]) -> io::Result<()> {
        self.dev.emit(events)
    }
}
