use std::io;
use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use evdev::raw_stream::RawDevice;
use evdev::{
    AbsoluteAxisType, BusType, Device, EventType, InputEvent, InputId, Key, Synchronization,
};
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use tracing::warn;

use crate::detector::TouchSample;
use crate::error::{Error, Result};
use crate::fsm::{ContactId, TouchFrame};
use crate::uinput::{TOUCHPAD_PRODUCT_ID, VENDOR_ID, WHEEL_PRODUCT_ID};

const INACTIVE_TRACKING_ID: i32 = -1;
const MAX_SUPPORTED_MT_SLOTS: usize = 256;
const TOUCHPAD_BUTTONS: [Key; 8] = [
    Key::BTN_LEFT,
    Key::BTN_RIGHT,
    Key::BTN_MIDDLE,
    Key::BTN_SIDE,
    Key::BTN_EXTRA,
    Key::BTN_FORWARD,
    Key::BTN_BACK,
    Key::BTN_TASK,
];

nix::ioctl_read_buf!(eviocgmtslots, b'E', 0x0a, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconciliationKind {
    LostSynchronization,
    LivenessCheck,
}

pub struct InputDevice {
    pub device: RawDevice,
    pub abs_x_min: i32,
    pub abs_x_max: i32,
    pub abs_y_min: i32,
    pub abs_y_max: i32,
    pub center_x: i32,
    pub center_y: i32,
    state: FrameState,
    pending_events: Vec<InputEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotState {
    tracking_id: i32,
    x: i32,
    y: i32,
}

impl Default for SlotState {
    fn default() -> Self {
        Self {
            tracking_id: INACTIVE_TRACKING_ID,
            x: 0,
            y: 0,
        }
    }
}

#[derive(Clone, Debug)]
struct FrameState {
    slots: Vec<SlotState>,
    current_slot: Option<usize>,
    contact: bool,
    pressed_buttons: u8,
}

#[derive(Clone, Debug)]
pub struct ContactSnapshot {
    slots: Vec<SlotState>,
    current_slot: Option<usize>,
    contact: bool,
    pressed_buttons: u8,
}

impl ContactSnapshot {
    pub fn primary(&self) -> TouchFrame {
        let active = self.contact.then(|| {
            self.slots
                .iter()
                .enumerate()
                .find(|(_, slot)| slot.tracking_id != INACTIVE_TRACKING_ID)
        });
        match active.flatten() {
            Some((slot, state)) => TouchFrame {
                contact: true,
                pos: Some(TouchSample {
                    x: state.x,
                    y: state.y,
                }),
                contact_id: Some(ContactId {
                    slot,
                    tracking_id: state.tracking_id,
                }),
            },
            None => TouchFrame {
                contact: false,
                pos: None,
                contact_id: None,
            },
        }
    }

    pub fn for_contact(&self, id: ContactId) -> TouchFrame {
        let active = self
            .slots
            .get(id.slot)
            .filter(|slot| self.contact && slot.tracking_id == id.tracking_id);
        match active {
            Some(state) => TouchFrame {
                contact: true,
                pos: Some(TouchSample {
                    x: state.x,
                    y: state.y,
                }),
                contact_id: Some(id),
            },
            None => TouchFrame {
                contact: false,
                pos: None,
                contact_id: Some(id),
            },
        }
    }

    pub fn contains(&self, id: ContactId) -> bool {
        self.slots
            .get(id.slot)
            .is_some_and(|slot| slot.tracking_id == id.tracking_id)
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn is_quiescent(&self) -> bool {
        !self.contact
            && self.pressed_buttons == 0
            && self
                .slots
                .iter()
                .all(|slot| slot.tracking_id == INACTIVE_TRACKING_ID)
    }

    pub(crate) fn tracking_ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.slots.iter().map(|slot| slot.tracking_id)
    }

    pub(crate) fn current_slot(&self) -> Option<usize> {
        self.current_slot
    }

    pub fn from_slot_values(
        slots: &[(i32, i32, i32)],
        current_slot: Option<usize>,
        contact: bool,
    ) -> io::Result<Self> {
        Self::from_slot_values_and_buttons(slots, current_slot, contact, &[])
    }

    pub fn from_slot_values_and_buttons(
        slots: &[(i32, i32, i32)],
        current_slot: Option<usize>,
        contact: bool,
        pressed_buttons: &[Key],
    ) -> io::Result<Self> {
        if slots.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "at least one MT slot is required",
            ));
        }
        if current_slot.is_some_and(|slot| slot >= slots.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "current MT slot is out of range",
            ));
        }
        Ok(Self {
            slots: slots
                .iter()
                .map(|&(tracking_id, x, y)| SlotState { tracking_id, x, y })
                .collect(),
            current_slot,
            contact,
            pressed_buttons: touchpad_button_mask_from_keys(pressed_buttons.iter().copied()),
        })
    }
}

pub struct PhysicalFrame {
    pub contacts: ContactSnapshot,
    pub events: Vec<InputEvent>,
    pub reconciled: bool,
}

impl PhysicalFrame {
    pub fn from_parts(
        contacts: ContactSnapshot,
        events: Vec<InputEvent>,
        reconciled: bool,
    ) -> Self {
        Self {
            contacts,
            events,
            reconciled,
        }
    }
}

pub struct GrabGuard {
    input: InputDevice,
    active: bool,
}

pub enum GrabAttempt {
    Stable(GrabGuard),
    Retry(InputDevice),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UngrabbedStartupAction {
    WaitForQuiescence,
    AttemptGrab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrabbedStartupAction {
    AcceptStableGrab,
    ReleaseAndRetry,
}

#[derive(Debug, Default)]
pub struct StartupCoordinator {
    stable_grab: bool,
    processor_initialized: bool,
}

impl StartupCoordinator {
    pub fn inspect_ungrabbed(&mut self, snapshot: &ContactSnapshot) -> UngrabbedStartupAction {
        self.stable_grab = false;
        self.processor_initialized = false;
        if snapshot.is_quiescent() {
            UngrabbedStartupAction::AttemptGrab
        } else {
            UngrabbedStartupAction::WaitForQuiescence
        }
    }

    pub fn inspect_grabbed(&mut self, snapshot: &ContactSnapshot) -> GrabbedStartupAction {
        self.processor_initialized = false;
        if snapshot.is_quiescent() {
            self.stable_grab = true;
            GrabbedStartupAction::AcceptStableGrab
        } else {
            self.stable_grab = false;
            GrabbedStartupAction::ReleaseAndRetry
        }
    }

    pub fn mark_processor_initialized(&mut self) {
        assert!(
            self.stable_grab,
            "processor cannot initialize before a stable quiescent grab"
        );
        self.processor_initialized = true;
    }

    pub fn may_notify_ready(&self) -> bool {
        self.stable_grab && self.processor_initialized
    }
}

impl Deref for GrabGuard {
    type Target = InputDevice;

    fn deref(&self) -> &Self::Target {
        &self.input
    }
}

impl DerefMut for GrabGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.input
    }
}

impl Drop for GrabGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        if let Err(source) = self.input.device.ungrab() {
            warn!(%source, "failed to release physical touchpad grab");
        }
    }
}

impl InputDevice {
    pub fn open(path: &Path) -> Result<Self> {
        let device = RawDevice::open(path).map_err(|source| Error::EvdevOpen {
            path: path.to_path_buf(),
            source,
        })?;
        set_nonblocking(device.as_raw_fd()).map_err(|source| Error::EvdevState {
            path: path.to_path_buf(),
            reason: format!("failed to enable O_NONBLOCK: {source}"),
        })?;
        validate_physical_source(path, device.input_id(), device.name())?;
        let slot_count = validate_raw_type_b(path, &device)?;
        let abs_state = device
            .get_abs_state()
            .map_err(|source| Error::EvdevRead { source })?;
        let xi = abs_state[AbsoluteAxisType::ABS_MT_POSITION_X.0 as usize];
        let yi = abs_state[AbsoluteAxisType::ABS_MT_POSITION_Y.0 as usize];
        let slot_info = abs_state[AbsoluteAxisType::ABS_MT_SLOT.0 as usize];
        let current_slot =
            validated_runtime_slot(slot_info.value, slot_count).map_err(|source| {
                Error::EvdevState {
                    path: path.to_path_buf(),
                    reason: source.to_string(),
                }
            })?;
        let key_state = device
            .get_key_state()
            .map_err(|source| Error::EvdevRead { source })?;
        let contact = key_state.contains(Key::BTN_TOUCH);
        let slots =
            query_all_slots(&device, slot_count).map_err(|source| Error::EvdevRead { source })?;
        let state = FrameState {
            slots,
            current_slot: Some(current_slot),
            contact,
            pressed_buttons: touchpad_button_mask(&key_state),
        };

        Ok(Self {
            device,
            abs_x_min: xi.minimum,
            abs_x_max: xi.maximum,
            abs_y_min: yi.minimum,
            abs_y_max: yi.maximum,
            center_x: (xi.minimum + xi.maximum) / 2,
            center_y: (yi.minimum + yi.maximum) / 2,
            state,
            pending_events: Vec::new(),
        })
    }

    pub fn grab_if_quiescent(mut self) -> Result<GrabAttempt> {
        self.device
            .grab()
            .map_err(|source| Error::Grab { source })?;
        // Do not read here. Events delivered after EVIOCGRAB belong to
        // the proxy lifecycle and must remain queued for normal
        // processing. The immediate ioctl snapshot is nonblocking.
        let refreshed = match self.query_frame_state() {
            Ok(state) => state,
            Err(source) => {
                if let Err(ungrab_source) = self.device.ungrab() {
                    warn!(
                        %ungrab_source,
                        "failed to release physical touchpad grab after state-query failure"
                    );
                }
                return Err(Error::EvdevRead { source });
            }
        };
        self.state = refreshed;
        self.pending_events.clear();
        if self.snapshot().is_quiescent() {
            Ok(GrabAttempt::Stable(GrabGuard {
                input: self,
                active: true,
            }))
        } else {
            self.device
                .ungrab()
                .map_err(|source| Error::Ungrab { source })?;
            Ok(GrabAttempt::Retry(self))
        }
    }

    pub fn refresh_ungrabbed_state(&mut self) -> io::Result<()> {
        drain_available_events(&mut self.device)?;
        self.pending_events.clear();
        self.state = self.query_frame_state()?;
        Ok(())
    }

    pub fn snapshot(&self) -> ContactSnapshot {
        self.state.snapshot()
    }

    pub fn find_by_name(regex_str: &str) -> Result<PathBuf> {
        let re = regex::Regex::new(regex_str).map_err(|source| Error::RegexInvalid {
            pattern: regex_str.to_string(),
            source,
        })?;
        let mut candidates = Vec::new();
        for (path, device) in evdev::enumerate() {
            let Some(name) = device.name() else {
                continue;
            };
            if re.is_match(name)
                && !is_virtual_source(&device.input_id(), name)
                && validate_sync_type_b(&path, &device).is_ok()
            {
                candidates.push((path, name.to_string()));
            }
        }
        select_candidate(regex_str, candidates)
    }

    pub fn poll_frames(&mut self) -> io::Result<Vec<PhysicalFrame>> {
        let fetched = match self.device.fetch_events() {
            Ok(events) => events.collect::<Vec<_>>(),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => return Ok(Vec::new()),
            Err(source) => return Err(source),
        };
        self.pending_events.extend(fetched);
        let mut frames = Vec::new();

        while let Some(end) = self.pending_events.iter().position(is_syn_report) {
            let events = self.pending_events.drain(..=end).collect::<Vec<_>>();
            if events.iter().any(is_syn_dropped) {
                frames.push(self.reconcile_frame(ReconciliationKind::LostSynchronization)?);
                continue;
            }
            for event in &events {
                self.state.apply_event(*event)?;
            }
            frames.push(PhysicalFrame {
                contacts: self.state.snapshot(),
                events,
                reconciled: false,
            });
        }
        Ok(frames)
    }

    pub fn reconcile_liveness_frame(&mut self) -> io::Result<PhysicalFrame> {
        self.reconcile_frame(ReconciliationKind::LivenessCheck)
    }

    fn reconcile_frame(&mut self, kind: ReconciliationKind) -> io::Result<PhysicalFrame> {
        let previous = self.state.clone();
        let refreshed = self.query_frame_state()?;
        let events = reconciliation_events(&previous, &refreshed, kind)?;
        self.state = refreshed;
        Ok(PhysicalFrame {
            contacts: self.state.snapshot(),
            events,
            reconciled: true,
        })
    }

    fn query_frame_state(&self) -> io::Result<FrameState> {
        let abs_state = self.device.get_abs_state()?;
        let current_slot = validated_runtime_slot(
            abs_state[AbsoluteAxisType::ABS_MT_SLOT.0 as usize].value,
            self.state.slots.len(),
        )?;
        let key_state = self.device.get_key_state()?;
        let contact = key_state.contains(Key::BTN_TOUCH);
        Ok(FrameState {
            slots: query_all_slots(&self.device, self.state.slots.len())?,
            current_slot: Some(current_slot),
            contact,
            pressed_buttons: touchpad_button_mask(&key_state),
        })
    }
}

fn validate_raw_type_b(path: &Path, device: &RawDevice) -> Result<usize> {
    validate_required_caps(
        path,
        device.supported_absolute_axes(),
        device.supported_keys(),
    )?;
    let abs_state = device
        .get_abs_state()
        .map_err(|source| Error::EvdevRead { source })?;
    let slot = abs_state[AbsoluteAxisType::ABS_MT_SLOT.0 as usize];
    slot_count_from_range(path, slot.minimum, slot.maximum)
}

fn validate_sync_type_b(path: &Path, device: &Device) -> Result<usize> {
    validate_required_caps(
        path,
        device.supported_absolute_axes(),
        device.supported_keys(),
    )?;
    let abs_state = device
        .get_abs_state()
        .map_err(|source| Error::EvdevRead { source })?;
    let slot = abs_state[AbsoluteAxisType::ABS_MT_SLOT.0 as usize];
    slot_count_from_range(path, slot.minimum, slot.maximum)
}

fn validate_required_caps(
    path: &Path,
    abs: Option<&evdev::AttributeSetRef<AbsoluteAxisType>>,
    keys: Option<&evdev::AttributeSetRef<Key>>,
) -> Result<()> {
    for (axis, name) in [
        (AbsoluteAxisType::ABS_MT_SLOT, "ABS_MT_SLOT"),
        (AbsoluteAxisType::ABS_MT_TRACKING_ID, "ABS_MT_TRACKING_ID"),
        (AbsoluteAxisType::ABS_MT_POSITION_X, "ABS_MT_POSITION_X"),
        (AbsoluteAxisType::ABS_MT_POSITION_Y, "ABS_MT_POSITION_Y"),
    ] {
        if !abs.is_some_and(|axes| axes.contains(axis)) {
            return Err(Error::EvdevMissingCap {
                path: path.to_path_buf(),
                capability: name,
            });
        }
    }
    if !keys.is_some_and(|set| set.contains(Key::BTN_TOUCH)) {
        return Err(Error::EvdevMissingCap {
            path: path.to_path_buf(),
            capability: "BTN_TOUCH",
        });
    }
    Ok(())
}

fn slot_count_from_range(path: &Path, minimum: i32, maximum: i32) -> Result<usize> {
    if minimum != 0 || maximum < minimum {
        return Err(Error::EvdevSlotRange {
            path: path.to_path_buf(),
            minimum,
            maximum,
            expected: "minimum 0 and maximum >= 0",
        });
    }
    let count = maximum as usize + 1;
    if count > MAX_SUPPORTED_MT_SLOTS {
        return Err(Error::EvdevSlotRange {
            path: path.to_path_buf(),
            minimum,
            maximum,
            expected: "at most 256 slots",
        });
    }
    Ok(count)
}

fn set_nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    let current = fcntl(fd, FcntlArg::F_GETFL)
        .map(OFlag::from_bits_truncate)
        .map_err(|errno| io::Error::from_raw_os_error(errno as i32))?;
    fcntl(fd, FcntlArg::F_SETFL(current | OFlag::O_NONBLOCK))
        .map(|_| ())
        .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
}

trait NonblockingEventSource {
    fn drain_batch(&mut self) -> io::Result<usize>;
}

impl NonblockingEventSource for RawDevice {
    fn drain_batch(&mut self) -> io::Result<usize> {
        self.fetch_events().map(Iterator::count)
    }
}

fn drain_available_events(source: &mut impl NonblockingEventSource) -> io::Result<usize> {
    let mut drained = 0;
    loop {
        match source.drain_batch() {
            Ok(0) => return Ok(drained),
            Ok(count) => drained += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(drained),
            Err(error) => return Err(error),
        }
    }
}

fn validated_runtime_slot(value: i32, slot_count: usize) -> io::Result<usize> {
    usize::try_from(value)
        .ok()
        .filter(|slot| *slot < slot_count)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ABS_MT_SLOT value {value} outside 0..{slot_count}"),
            )
        })
}

fn query_all_slots(device: &RawDevice, slot_count: usize) -> io::Result<Vec<SlotState>> {
    let tracking_ids = query_slot_values(device, slot_count, AbsoluteAxisType::ABS_MT_TRACKING_ID)?;
    let xs = query_slot_values(device, slot_count, AbsoluteAxisType::ABS_MT_POSITION_X)?;
    let ys = query_slot_values(device, slot_count, AbsoluteAxisType::ABS_MT_POSITION_Y)?;
    Ok((0..slot_count)
        .map(|slot| SlotState {
            tracking_id: tracking_ids[slot],
            x: xs[slot],
            y: ys[slot],
        })
        .collect())
}

fn query_slot_values(
    device: &RawDevice,
    slot_count: usize,
    axis: AbsoluteAxisType,
) -> io::Result<Vec<i32>> {
    let mut request = vec![0_i32; slot_count + 1];
    request[0] = axis.0 as i32;
    // SAFETY: EVIOCGMTSLOTS expects one u32 axis code followed by
    // `slot_count` i32 values. `i32` has the same 4-byte layout as the
    // kernel's u32 code field, the slice is writable for its full
    // ioctl-encoded length, and the FD remains owned by `device`.
    unsafe { eviocgmtslots(device.as_raw_fd(), &mut request) }
        .map_err(|errno| io::Error::from_raw_os_error(errno as i32))?;
    Ok(request.split_off(1))
}

fn touchpad_button_mask(keys: &evdev::AttributeSet<Key>) -> u8 {
    touchpad_button_mask_from_keys(
        TOUCHPAD_BUTTONS
            .into_iter()
            .filter(|key| keys.contains(*key)),
    )
}

fn touchpad_button_mask_from_keys(keys: impl IntoIterator<Item = Key>) -> u8 {
    let mut mask = 0_u8;
    for key in keys {
        if let Some(index) = TOUCHPAD_BUTTONS
            .iter()
            .position(|candidate| *candidate == key)
        {
            mask |= 1 << index;
        }
    }
    mask
}

fn reconciliation_events(
    previous: &FrameState,
    refreshed: &FrameState,
    kind: ReconciliationKind,
) -> io::Result<Vec<InputEvent>> {
    if previous.slots.len() != refreshed.slots.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "MT slot count changed during reconciliation",
        ));
    }
    let final_slot = refreshed.current_slot.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "refreshed Type B state has no selected MT slot",
        )
    })?;
    if final_slot >= refreshed.slots.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "refreshed current MT slot {final_slot} outside 0..{}",
                refreshed.slots.len()
            ),
        ));
    }

    let mut events = Vec::new();
    for (slot, (old, new)) in previous.slots.iter().zip(&refreshed.slots).enumerate() {
        let identity_changed = old.tracking_id != new.tracking_id;
        let rebuild_position = new.tracking_id != INACTIVE_TRACKING_ID
            && (identity_changed || kind == ReconciliationKind::LostSynchronization);
        if identity_changed || rebuild_position {
            events.push(abs_event(AbsoluteAxisType::ABS_MT_SLOT, slot as i32));
        }
        if identity_changed {
            if old.tracking_id != INACTIVE_TRACKING_ID {
                events.push(abs_event(
                    AbsoluteAxisType::ABS_MT_TRACKING_ID,
                    INACTIVE_TRACKING_ID,
                ));
            }
            if new.tracking_id != INACTIVE_TRACKING_ID {
                events.push(abs_event(
                    AbsoluteAxisType::ABS_MT_TRACKING_ID,
                    new.tracking_id,
                ));
            }
        }
        if rebuild_position {
            events.push(abs_event(AbsoluteAxisType::ABS_MT_POSITION_X, new.x));
            events.push(abs_event(AbsoluteAxisType::ABS_MT_POSITION_Y, new.y));
        }
    }
    if previous.contact != refreshed.contact {
        events.push(InputEvent::new(
            EventType::KEY,
            Key::BTN_TOUCH.0,
            i32::from(refreshed.contact),
        ));
    }
    events.push(abs_event(AbsoluteAxisType::ABS_MT_SLOT, final_slot as i32));
    events.push(InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));
    Ok(events)
}

fn abs_event(axis: AbsoluteAxisType, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE, axis.0, value)
}

fn is_syn_report(event: &InputEvent) -> bool {
    event.event_type() == EventType::SYNCHRONIZATION
        && event.code() == Synchronization::SYN_REPORT.0
}

fn is_syn_dropped(event: &InputEvent) -> bool {
    event.event_type() == EventType::SYNCHRONIZATION
        && event.code() == Synchronization::SYN_DROPPED.0
}

fn select_candidate(regex_str: &str, mut candidates: Vec<(PathBuf, String)>) -> Result<PathBuf> {
    match candidates.len() {
        0 => Err(Error::DeviceNotFound {
            regex: regex_str.to_string(),
        }),
        1 => Ok(candidates.pop().unwrap().0),
        _ => {
            let candidates = candidates
                .into_iter()
                .map(|(path, name)| format!("  {} — {name}", path.display()))
                .collect::<Vec<_>>()
                .join("\n");
            Err(Error::DeviceAmbiguous {
                regex: regex_str.to_string(),
                candidates,
            })
        }
    }
}

fn validate_physical_source(path: &Path, id: InputId, name: Option<&str>) -> Result<()> {
    let name = name.unwrap_or("<unnamed>");
    if is_virtual_source(&id, name) {
        return Err(Error::VirtualInputSource {
            path: path.to_path_buf(),
            name: name.to_string(),
        });
    }
    Ok(())
}

fn is_virtual_source(id: &InputId, name: &str) -> bool {
    id.bus_type() == BusType::BUS_VIRTUAL
        || (id.vendor() == VENDOR_ID
            && matches!(id.product(), WHEEL_PRODUCT_ID | TOUCHPAD_PRODUCT_ID))
        || name.contains("(letsnote-wheelpad)")
}

impl FrameState {
    fn apply_event(&mut self, event: InputEvent) -> io::Result<()> {
        if event.event_type() == EventType::KEY && event.code() == Key::BTN_TOUCH.0 {
            self.contact = event.value() != 0;
            return Ok(());
        }
        if event.event_type() == EventType::KEY {
            let key = Key(event.code());
            if let Some(index) = TOUCHPAD_BUTTONS
                .iter()
                .position(|candidate| *candidate == key)
            {
                if event.value() == 0 {
                    self.pressed_buttons &= !(1 << index);
                } else {
                    self.pressed_buttons |= 1 << index;
                }
            }
            return Ok(());
        }
        if event.event_type() != EventType::ABSOLUTE {
            return Ok(());
        }
        let axis = AbsoluteAxisType(event.code());
        if axis == AbsoluteAxisType::ABS_MT_SLOT {
            self.current_slot = None;
            self.current_slot = Some(validated_runtime_slot(event.value(), self.slots.len())?);
            return Ok(());
        }
        let Some(slot) = self.current_slot else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("received {axis:?} without a valid selected MT slot"),
            ));
        };
        match axis {
            AbsoluteAxisType::ABS_MT_TRACKING_ID => {
                self.slots[slot].tracking_id = event.value();
            }
            AbsoluteAxisType::ABS_MT_POSITION_X => self.slots[slot].x = event.value(),
            AbsoluteAxisType::ABS_MT_POSITION_Y => self.slots[slot].y = event.value(),
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self) -> ContactSnapshot {
        ContactSnapshot {
            slots: self.slots.clone(),
            current_slot: self.current_slot,
            contact: self.contact,
            pressed_buttons: self.pressed_buttons,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use nix::unistd::{close, pipe};

    use super::*;

    enum DrainStep {
        Events(usize),
        WouldBlock,
        Error,
    }

    struct FakeEventSource {
        steps: VecDeque<DrainStep>,
    }

    impl FakeEventSource {
        fn new(steps: impl IntoIterator<Item = DrainStep>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
            }
        }
    }

    impl NonblockingEventSource for FakeEventSource {
        fn drain_batch(&mut self) -> io::Result<usize> {
            match self.steps.pop_front().unwrap_or(DrainStep::WouldBlock) {
                DrainStep::Events(count) => Ok(count),
                DrainStep::WouldBlock => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                DrainStep::Error => Err(io::Error::other("read failed")),
            }
        }
    }

    #[derive(Debug)]
    struct VirtualTypeB {
        slots: Vec<SlotState>,
        current_slot: Option<usize>,
    }

    impl VirtualTypeB {
        fn from_state(state: &FrameState) -> Self {
            Self {
                slots: state.slots.clone(),
                current_slot: state.current_slot,
            }
        }

        fn apply(&mut self, events: &[InputEvent]) {
            for event in events {
                if event.event_type() != EventType::ABSOLUTE {
                    continue;
                }
                let axis = AbsoluteAxisType(event.code());
                if axis == AbsoluteAxisType::ABS_MT_SLOT {
                    self.current_slot =
                        Some(validated_runtime_slot(event.value(), self.slots.len()).unwrap());
                    continue;
                }
                let slot = self
                    .current_slot
                    .expect("virtual device has a selected slot");
                match axis {
                    AbsoluteAxisType::ABS_MT_TRACKING_ID => {
                        self.slots[slot].tracking_id = event.value();
                    }
                    AbsoluteAxisType::ABS_MT_POSITION_X => {
                        self.slots[slot].x = event.value();
                    }
                    AbsoluteAxisType::ABS_MT_POSITION_Y => {
                        self.slots[slot].y = event.value();
                    }
                    _ => {}
                }
            }
        }
    }

    fn state(slot_count: usize) -> FrameState {
        FrameState {
            slots: vec![SlotState::default(); slot_count],
            current_slot: Some(0),
            contact: true,
            pressed_buttons: 0,
        }
    }

    fn set_contact(state: &mut FrameState, slot: usize, tracking_id: i32, x: i32, y: i32) {
        state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_SLOT, slot as i32))
            .unwrap();
        state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_TRACKING_ID, tracking_id))
            .unwrap();
        state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_POSITION_X, x))
            .unwrap();
        state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_POSITION_Y, y))
            .unwrap();
    }

    #[test]
    fn startup_detects_every_non_quiescent_input_class() {
        let active_tracking =
            ContactSnapshot::from_slot_values(&[(-1, 0, 0), (41, 800, 500)], Some(1), false)
                .unwrap();
        let touch = ContactSnapshot::from_slot_values(&[(-1, 0, 0); 2], Some(0), true).unwrap();
        let button = ContactSnapshot::from_slot_values_and_buttons(
            &[(-1, 0, 0); 2],
            Some(0),
            false,
            &[Key::BTN_LEFT],
        )
        .unwrap();
        let mut coordinator = StartupCoordinator::default();

        for snapshot in [&active_tracking, &touch, &button] {
            assert!(!snapshot.is_quiescent());
            assert_eq!(
                coordinator.inspect_ungrabbed(snapshot),
                UngrabbedStartupAction::WaitForQuiescence
            );
        }
    }

    #[test]
    fn startup_retries_when_input_activates_during_grab_window() {
        let quiet = ContactSnapshot::from_slot_values(&[(-1, 0, 0); 2], Some(0), false).unwrap();
        let raced = ContactSnapshot::from_slot_values(&[(-1, 0, 0), (41, 800, 500)], Some(1), true)
            .unwrap();
        let mut coordinator = StartupCoordinator::default();

        assert_eq!(
            coordinator.inspect_ungrabbed(&quiet),
            UngrabbedStartupAction::AttemptGrab
        );
        assert_eq!(
            coordinator.inspect_grabbed(&raced),
            GrabbedStartupAction::ReleaseAndRetry
        );
        assert!(!coordinator.may_notify_ready());
        assert_eq!(
            coordinator.inspect_ungrabbed(&raced),
            UngrabbedStartupAction::WaitForQuiescence
        );
        assert_eq!(
            coordinator.inspect_ungrabbed(&quiet),
            UngrabbedStartupAction::AttemptGrab
        );
        assert_eq!(
            coordinator.inspect_grabbed(&quiet),
            GrabbedStartupAction::AcceptStableGrab
        );
    }

    #[test]
    fn startup_ready_requires_stable_grab_and_processor_initialization() {
        let quiet = ContactSnapshot::from_slot_values(&[(-1, 0, 0); 2], Some(0), false).unwrap();
        let mut coordinator = StartupCoordinator::default();

        assert!(!coordinator.may_notify_ready());
        coordinator.inspect_ungrabbed(&quiet);
        assert!(!coordinator.may_notify_ready());
        coordinator.inspect_grabbed(&quiet);
        assert!(!coordinator.may_notify_ready());
        coordinator.mark_processor_initialized();
        assert!(coordinator.may_notify_ready());
    }

    #[test]
    fn tracked_contact_does_not_switch_to_a_lower_slot() {
        let mut state = state(4);
        set_contact(&mut state, 3, 30, 800, 500);
        let tracked = state.snapshot().primary().contact_id.unwrap();
        set_contact(&mut state, 1, 10, 200, 300);
        assert_eq!(
            state.snapshot().for_contact(tracked).pos,
            Some(TouchSample { x: 800, y: 500 })
        );
    }

    #[test]
    fn slot_boundary_accepts_highest_and_rejects_first_outside() {
        let mut state = state(4);
        assert!(state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_SLOT, 3))
            .is_ok());
        assert!(state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_SLOT, 4))
            .is_err());
        assert_eq!(state.current_slot, None);
        assert!(state
            .apply_event(abs_event(AbsoluteAxisType::ABS_MT_POSITION_X, 99))
            .is_err());
    }

    #[test]
    fn advertised_slot_range_is_validated() {
        let path = Path::new("/dev/input/test");
        assert_eq!(slot_count_from_range(path, 0, 255).unwrap(), 256);
        assert!(slot_count_from_range(path, 0, 256).is_err());
        assert!(slot_count_from_range(path, 1, 4).is_err());
        assert!(slot_count_from_range(path, 0, -1).is_err());
    }

    #[test]
    fn opening_flags_preserve_existing_status_and_add_nonblocking() {
        let (read_fd, write_fd) = pipe().unwrap();
        let initial = OFlag::from_bits_truncate(fcntl(read_fd, FcntlArg::F_GETFL).unwrap());
        fcntl(read_fd, FcntlArg::F_SETFL(initial | OFlag::O_ASYNC)).unwrap();

        set_nonblocking(read_fd).unwrap();
        let final_flags = OFlag::from_bits_truncate(fcntl(read_fd, FcntlArg::F_GETFL).unwrap());

        assert!(final_flags.contains(OFlag::O_NONBLOCK));
        assert!(final_flags.contains(OFlag::O_ASYNC));
        close(read_fd).unwrap();
        close(write_fd).unwrap();
    }

    #[test]
    fn ungrabbed_drain_stops_cleanly_at_would_block() {
        let mut source = FakeEventSource::new([
            DrainStep::Events(2),
            DrainStep::Events(1),
            DrainStep::WouldBlock,
        ]);
        assert_eq!(drain_available_events(&mut source).unwrap(), 3);
        assert!(source.steps.is_empty());
    }

    #[test]
    fn quiescent_startup_with_no_events_does_not_wait_for_a_read() {
        let mut source = FakeEventSource::new([DrainStep::WouldBlock]);
        assert_eq!(drain_available_events(&mut source).unwrap(), 0);
        let quiet = ContactSnapshot::from_slot_values(&[(-1, 0, 0); 2], Some(1), false).unwrap();
        let mut coordinator = StartupCoordinator::default();
        assert_eq!(
            coordinator.inspect_ungrabbed(&quiet),
            UngrabbedStartupAction::AttemptGrab
        );
    }

    #[test]
    fn ungrabbed_drain_propagates_real_read_errors() {
        let mut source = FakeEventSource::new([DrainStep::Error]);
        assert_eq!(
            drain_available_events(&mut source).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn reconciliation_ends_old_identity_and_starts_reused_slot() {
        let mut old = state(2);
        set_contact(&mut old, 1, 20, 700, 500);
        let mut new = state(2);
        set_contact(&mut new, 1, 31, 640, 480);
        let events =
            reconciliation_events(&old, &new, ReconciliationKind::LostSynchronization).unwrap();
        let semantics = events
            .iter()
            .map(|event| (event.event_type().0, event.code(), event.value()))
            .collect::<Vec<_>>();
        assert!(semantics.contains(&(
            EventType::ABSOLUTE.0,
            AbsoluteAxisType::ABS_MT_TRACKING_ID.0,
            -1
        )));
        assert!(semantics.contains(&(
            EventType::ABSOLUTE.0,
            AbsoluteAxisType::ABS_MT_TRACKING_ID.0,
            31
        )));
        assert!(semantics.contains(&(
            EventType::ABSOLUTE.0,
            AbsoluteAxisType::ABS_MT_POSITION_X.0,
            640
        )));
    }

    #[test]
    fn reconciliation_restores_virtual_current_slot_for_following_positions() {
        let mut old = state(2);
        set_contact(&mut old, 0, 10, 100, 200);
        old.current_slot = Some(0);
        let mut refreshed = old.clone();
        set_contact(&mut refreshed, 1, 20, 700, 500);
        refreshed.current_slot = Some(0);

        let events =
            reconciliation_events(&old, &refreshed, ReconciliationKind::LostSynchronization)
                .unwrap();
        let restored_slot = events[events.len() - 2];
        assert_eq!(restored_slot.event_type(), EventType::ABSOLUTE);
        assert_eq!(restored_slot.code(), AbsoluteAxisType::ABS_MT_SLOT.0);
        assert_eq!(restored_slot.value(), 0);
        assert!(is_syn_report(events.last().unwrap()));

        let mut virtual_device = VirtualTypeB::from_state(&old);
        virtual_device.apply(&events);
        virtual_device.apply(&[
            abs_event(AbsoluteAxisType::ABS_MT_POSITION_X, 150),
            abs_event(AbsoluteAxisType::ABS_MT_POSITION_Y, 250),
        ]);
        assert_eq!(virtual_device.current_slot, Some(0));
        assert_eq!(
            (virtual_device.slots[0].x, virtual_device.slots[0].y),
            (150, 250)
        );
        assert_eq!(
            (virtual_device.slots[1].x, virtual_device.slots[1].y),
            (700, 500)
        );
    }

    #[test]
    fn lost_sync_rebuilds_changed_positions_for_surviving_identity() {
        let mut old = state(2);
        set_contact(&mut old, 1, 20, 700, 500);
        let mut refreshed = old.clone();
        refreshed.slots[1].x = 640;
        refreshed.slots[1].y = 480;

        let events =
            reconciliation_events(&old, &refreshed, ReconciliationKind::LostSynchronization)
                .unwrap();
        let mut virtual_device = VirtualTypeB::from_state(&old);
        virtual_device.apply(&events);

        assert_eq!(virtual_device.slots[1].tracking_id, 20);
        assert_eq!(
            (virtual_device.slots[1].x, virtual_device.slots[1].y),
            (640, 480)
        );
    }

    #[test]
    fn lost_sync_rebuilds_unchanged_positions_for_every_active_slot() {
        let mut old = state(3);
        set_contact(&mut old, 0, 10, 100, 200);
        set_contact(&mut old, 2, 30, 700, 500);
        old.current_slot = Some(1);
        let refreshed = old.clone();

        let events =
            reconciliation_events(&old, &refreshed, ReconciliationKind::LostSynchronization)
                .unwrap();
        let positions = events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::ABSOLUTE
                    && matches!(
                        AbsoluteAxisType(event.code()),
                        AbsoluteAxisType::ABS_MT_POSITION_X | AbsoluteAxisType::ABS_MT_POSITION_Y
                    )
            })
            .count();

        assert_eq!(positions, 4);
        let restored_slot = events[events.len() - 2];
        assert_eq!(restored_slot.event_type(), EventType::ABSOLUTE);
        assert_eq!(restored_slot.code(), AbsoluteAxisType::ABS_MT_SLOT.0);
        assert_eq!(restored_slot.value(), 1);
    }

    #[test]
    fn lost_sync_rebuilds_changed_and_surviving_identities_together() {
        let mut old = state(3);
        set_contact(&mut old, 0, 10, 100, 200);
        set_contact(&mut old, 2, 30, 700, 500);
        let mut refreshed = old.clone();
        refreshed.slots[0] = SlotState {
            tracking_id: 11,
            x: 120,
            y: 220,
        };
        refreshed.slots[2].x = 720;
        refreshed.slots[2].y = 520;
        refreshed.current_slot = Some(2);

        let events =
            reconciliation_events(&old, &refreshed, ReconciliationKind::LostSynchronization)
                .unwrap();
        let mut virtual_device = VirtualTypeB::from_state(&old);
        virtual_device.apply(&events);

        assert_eq!(virtual_device.slots, refreshed.slots);
        assert_eq!(virtual_device.current_slot, refreshed.current_slot);
    }

    #[test]
    fn reconciliation_rejects_missing_refreshed_current_slot() {
        let old = state(2);
        let mut refreshed = old.clone();
        refreshed.current_slot = None;
        assert!(
            reconciliation_events(&old, &refreshed, ReconciliationKind::LostSynchronization)
                .is_err()
        );
    }

    #[test]
    fn virtual_source_detection_covers_bus_ids_and_name() {
        let physical = InputId::new(BusType::BUS_I2C, 1, 2, 1);
        let virtual_bus = InputId::new(BusType::BUS_VIRTUAL, 1, 2, 1);
        let own_ids = InputId::new(BusType::BUS_I2C, VENDOR_ID, TOUCHPAD_PRODUCT_ID, 1);
        assert!(!is_virtual_source(&physical, "Synaptics TM3562"));
        assert!(is_virtual_source(&virtual_bus, "Synaptics TM3562"));
        assert!(is_virtual_source(&own_ids, "renamed device"));
        assert!(is_virtual_source(
            &physical,
            "Synaptics TM3562 (letsnote-wheelpad)"
        ));
    }

    #[test]
    fn multiple_supported_candidates_are_reported() {
        let result = select_candidate(
            "TM3562",
            vec![
                (PathBuf::from("/dev/input/event4"), "Touchpad A".into()),
                (PathBuf::from("/dev/input/event7"), "Touchpad B".into()),
            ],
        );
        assert!(matches!(result, Err(Error::DeviceAmbiguous { .. })));
    }
}
