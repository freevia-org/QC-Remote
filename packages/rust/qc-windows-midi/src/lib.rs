use std::time::{Duration, Instant};

#[derive(Default)]
pub struct PerformanceMidi {
    device_id: Option<String>,
    last_sent: Option<Instant>,
    handle: Option<usize>,
    endpoint: Option<String>,
}

pub struct PerformanceMidiReceipt {
    pub endpoint: String,
    pub dispatch_latency_ms: f64,
    pub throttle_delay_ms: f64,
}

impl PerformanceMidi {
    /// Bind MIDI to the same physical Windows device container as the HID
    /// session. Never fall back to a similarly named output on another unit.
    pub fn send_for_device(
        &mut self,
        device_id: &str,
        controller: u8,
        value: u8,
    ) -> Result<PerformanceMidiReceipt, String> {
        self.warm_up_for_device(device_id)?;
        self.send(controller, value)
    }

    pub fn warm_up_for_device(&mut self, device_id: &str) -> Result<String, String> {
        if self.device_id.as_deref() != Some(device_id) || self.handle.is_none() {
            let target = hid_container(device_id)?;
            close_qc_midi_output(self.handle.take());
            self.endpoint = None;
            self.device_id = None;
            let (handle, endpoint) = open_matching_qc_midi_output(Some(target))?;
            self.handle = Some(handle);
            self.endpoint = Some(endpoint);
            self.device_id = Some(device_id.to_owned());
        }
        Ok(self
            .endpoint
            .clone()
            .expect("selected MIDI output has a name"))
    }

    pub fn warm_up(&mut self) -> Result<String, String> {
        if self.handle.is_none() {
            let (handle, endpoint) = open_qc_midi_output()?;
            self.handle = Some(handle);
            self.endpoint = Some(endpoint);
        }
        Ok(self
            .endpoint
            .clone()
            .unwrap_or_else(|| "Quad Cortex MIDI".into()))
    }

    pub fn send(&mut self, controller: u8, value: u8) -> Result<PerformanceMidiReceipt, String> {
        self.send_raw(control_change_message(controller, value))
    }

    fn send_raw(&mut self, message: u32) -> Result<PerformanceMidiReceipt, String> {
        let queued_at = Instant::now();
        let mut throttle_delay = Duration::ZERO;
        if let Some(last_sent) = self.last_sent {
            let elapsed = last_sent.elapsed();
            let gap = Duration::from_millis(quadcortex::profile::PERFORMANCE_MIDI_GAP_MS);
            if elapsed < gap {
                throttle_delay = gap - elapsed;
                // Windows' ordinary Sleep timer can overshoot this 8 ms device
                // pacing interval by tens of milliseconds. The wait is short,
                // bounded, and only occurs for back-to-back performance MIDI,
                // so keep it precise instead of injecting audible control lag.
                let deadline = Instant::now() + throttle_delay;
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
            }
        }
        self.warm_up()?;
        let handle = self
            .handle
            .ok_or_else(|| "Quad Cortex MIDI initialization returned no handle".to_string())?;
        if let Err(error) = send_qc_midi_short(handle, message) {
            close_qc_midi_output(self.handle.take());
            self.endpoint = None;
            return Err(error);
        }
        self.last_sent = Some(Instant::now());
        Ok(PerformanceMidiReceipt {
            endpoint: self
                .endpoint
                .clone()
                .unwrap_or_else(|| "Quad Cortex MIDI".into()),
            dispatch_latency_ms: queued_at.elapsed().as_secs_f64() * 1000.0,
            throttle_delay_ms: throttle_delay.as_secs_f64() * 1000.0,
        })
    }
}

fn control_change_message(controller: u8, value: u8) -> u32 {
    u32::from(quadcortex::profile::MIDI_CONTROL_CHANGE_STATUS)
        | ((controller as u32) << 8)
        | ((value as u32) << 16)
}

impl Drop for PerformanceMidi {
    fn drop(&mut self) {
        close_qc_midi_output(self.handle.take());
    }
}

#[cfg(windows)]
fn open_qc_midi_output() -> Result<(usize, String), String> {
    open_matching_qc_midi_output(None)
}

#[cfg(windows)]
fn hid_container(device_id: &str) -> Result<[u8; 16], String> {
    if device_id.is_empty()
        || !device_id.len().is_multiple_of(2)
        || device_id.len() > 8192
        || !device_id.is_ascii()
    {
        return Err("Invalid Quad Cortex device identity".into());
    }
    let bytes: Result<Vec<_>, _> = (0..device_id.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&device_id[index..index + 2], 16))
        .collect();
    let path = String::from_utf8(bytes.map_err(|_| "Invalid Quad Cortex device identity")?)
        .map_err(|_| "Invalid Quad Cortex device path")?;
    let wide: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
    interface_container(&wide)
}

#[cfg(windows)]
fn midi_container(device_id: u32) -> Result<[u8; 16], String> {
    use windows_sys::Win32::Media::Audio::midiOutMessage;
    use windows_sys::Win32::Media::Multimedia::{
        DRV_QUERYDEVICEINTERFACE, DRV_QUERYDEVICEINTERFACESIZE,
    };
    let mut bytes = 0_u32;
    // For these system-intercepted messages WinMM accepts the numeric device
    // index in the handle slot. No MIDI data is sent by either query.
    // SAFETY: the pointer argument is the address of the live local `bytes`, which is what
    // DRV_QUERYDEVICEINTERFACESIZE fills in (a byte count). The first argument is a device
    // index, not a pointer, and an index WinMM does not know is reported through the return
    // value.
    if unsafe {
        midiOutMessage(
            device_id as usize as _,
            DRV_QUERYDEVICEINTERFACESIZE,
            &mut bytes as *mut _ as usize,
            0,
        )
    } != 0
        || !(2..=65536).contains(&bytes)
        || !bytes.is_multiple_of(2)
    {
        return Err("Could not identify the MIDI device interface".into());
    }
    let mut path = vec![0_u16; bytes as usize / 2];
    // SAFETY: `path` holds `bytes / 2` u16 units, exactly `bytes` bytes, and is passed as the
    // writable output buffer together with its size, which is what DRV_QUERYDEVICEINTERFACE
    // expects.
    if unsafe {
        midiOutMessage(
            device_id as usize as _,
            DRV_QUERYDEVICEINTERFACE,
            path.as_mut_ptr() as usize,
            bytes as usize,
        )
    } != 0
    {
        return Err("Could not read the MIDI device interface".into());
    }
    if path.last() != Some(&0) {
        return Err("Unterminated MIDI device interface".into());
    }
    interface_container(&path)
}

#[cfg(windows)]
fn interface_container(path: &[u16]) -> Result<[u8; 16], String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_Device_Interface_PropertyW, CM_Locate_DevNodeW, CR_SUCCESS,
    };
    use windows_sys::Win32::Devices::Properties::{
        DEVPKEY_Device_ContainerId, DEVPKEY_Device_InstanceId, DEVPROP_TYPE_GUID,
        DEVPROP_TYPE_STRING,
    };
    let mut kind = 0;
    let mut bytes = 0;
    // SAFETY: `path` is a NUL-terminated UTF-16 slice (hid_container appends the terminator and
    // midi_container checks for it before calling). `kind` and `bytes` are live locals and the
    // property key is a static. A null buffer with `bytes == 0` is the documented sizing call;
    // its CR_BUFFER_SMALL result is deliberately ignored and `bytes` is validated below.
    unsafe {
        CM_Get_Device_Interface_PropertyW(
            path.as_ptr(),
            &DEVPKEY_Device_InstanceId,
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
            0,
        );
    }
    if !(2..=65536).contains(&bytes) || bytes % 2 != 0 {
        return Err("Device interface has no instance identity".into());
    }
    let mut instance = vec![0_u16; bytes as usize / 2];
    // SAFETY: `instance` holds `bytes / 2` u16 units, exactly `bytes` bytes, and is passed as the
    // writable buffer with that size in `bytes`; `path` is NUL-terminated as above.
    if unsafe {
        CM_Get_Device_Interface_PropertyW(
            path.as_ptr(),
            &DEVPKEY_Device_InstanceId,
            &mut kind,
            instance.as_mut_ptr().cast(),
            &mut bytes,
            0,
        )
    } != CR_SUCCESS
        || kind != DEVPROP_TYPE_STRING
        || instance.last() != Some(&0)
    {
        return Err("Could not read the device instance".into());
    }
    let mut node = 0;
    // SAFETY: `instance` is a NUL-terminated UTF-16 string (its last unit was checked to be 0
    // above) and `node` is a writable local.
    if unsafe { CM_Locate_DevNodeW(&mut node, instance.as_ptr(), 0) } != CR_SUCCESS {
        return Err("Device instance is not present".into());
    }
    let mut container = [0_u8; 16];
    bytes = 16;
    // SAFETY: `node` came from the successful CM_Locate_DevNodeW above. `container` is a
    // writable 16-byte buffer, `bytes` was set to 16 as its size, and `kind` is a live local.
    if unsafe {
        CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_ContainerId,
            &mut kind,
            container.as_mut_ptr(),
            &mut bytes,
            0,
        )
    } != CR_SUCCESS
        || kind != DEVPROP_TYPE_GUID
        || bytes != 16
        || container == [0; 16]
    {
        return Err("Device has no physical container identity".into());
    }
    Ok(container)
}

#[cfg(not(windows))]
fn hid_container(_: &str) -> Result<[u8; 16], String> {
    Err("Windows MIDI is unavailable".into())
}
#[cfg(not(windows))]
fn open_matching_qc_midi_output(_: Option<[u8; 16]>) -> Result<(usize, String), String> {
    Err("Windows MIDI is unavailable".into())
}

#[cfg(windows)]
fn open_matching_qc_midi_output(target: Option<[u8; 16]>) -> Result<(usize, String), String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Media::Audio::{
        midiOutGetDevCapsW, midiOutGetNumDevs, midiOutOpen, HMIDIOUT, MIDIOUTCAPSW,
    };

    let mut endpoint = None;
    // SAFETY: takes no arguments and reads no memory of ours.
    for device_id in 0..unsafe { midiOutGetNumDevs() } {
        let mut caps = MIDIOUTCAPSW::default();
        // SAFETY: `caps` is a live, writable MIDIOUTCAPSW and its size_of is passed. `device_id` is
        // below midiOutGetNumDevs, and a failure is reported through the return value.
        let result = unsafe {
            midiOutGetDevCapsW(
                device_id as usize,
                &mut caps,
                std::mem::size_of::<MIDIOUTCAPSW>() as u32,
            )
        };
        if result != 0 {
            continue;
        }
        let name_units = caps.szPname;
        let end = name_units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name_units.len());
        let name = String::from_utf16_lossy(&name_units[..end]);
        if name.to_lowercase().contains("quad cortex")
            && target.is_none_or(|target| {
                midi_container(device_id).is_ok_and(|container| container == target)
            })
        {
            endpoint = Some((device_id, name));
            break;
        }
    }
    let (device_id, name) = endpoint.ok_or_else(|| {
        "Quad Cortex Windows MIDI output was not found. Enable MIDI over USB and reconnect."
            .to_string()
    })?;
    let mut handle: HMIDIOUT = null_mut();
    // SAFETY: `handle` is a live, writable slot for the HMIDIOUT. No callback, instance data or
    // flags are passed, so WinMM keeps no pointer into this process. On success the handle
    // stays open until `close_qc_midi_output` closes it.
    let opened = unsafe { midiOutOpen(&mut handle, device_id, 0, 0, 0) };
    if opened != 0 {
        return Err(format!(
            "Could not open the Quad Cortex MIDI output (Windows MIDI error {opened})."
        ));
    }
    Ok((handle as usize, name))
}

#[cfg(windows)]
fn send_qc_midi_short(handle: usize, message: u32) -> Result<(), String> {
    use windows_sys::Win32::Media::Audio::midiOutShortMsg;

    // SAFETY: the only caller (send_raw) passes the handle midiOutOpen produced. It is closed only
    // by `close_qc_midi_output`, which every caller feeds from `Option::take` on the stored
    // handle, so a stored handle is still open here. A short message carries no pointer, and
    // `&mut self` upstream means the handle is not used concurrently.
    let sent = unsafe { midiOutShortMsg(handle as _, message) };
    if sent != 0 {
        return Err(format!(
            "Could not send the Quad Cortex MIDI command (Windows MIDI error {sent})."
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_qc_midi_output() -> Result<(usize, String), String> {
    Err("QC performance control currently requires Windows MIDI.".into())
}

#[cfg(not(windows))]
fn send_qc_midi_short(_handle: usize, _message: u32) -> Result<(), String> {
    Err("QC performance control currently requires Windows MIDI.".into())
}

#[cfg(windows)]
fn close_qc_midi_output(handle: Option<usize>) {
    use windows_sys::Win32::Media::Audio::midiOutClose;

    if let Some(handle) = handle {
        // SAFETY: `handle` was returned by midiOutOpen and every caller passes `Option::take` of
        // the stored handle, so each opened handle is closed at most once. Only short messages
        // are ever sent, so no long-message buffer is outstanding on it.
        unsafe { midiOutClose(handle as _) };
    }
}

#[cfg(not(windows))]
fn close_qc_midi_output(_handle: Option<usize>) {}

#[cfg(test)]
mod tests {
    use super::control_change_message;

    #[test]
    fn control_change_packing_matches_press_footswitch_bytes() {
        assert_eq!(control_change_message(35, 127), 0x007f_23b0);
        assert_eq!(control_change_message(42, 127), 0x007f_2ab0);
    }

    #[test]
    fn control_change_packing_matches_tap_tempo_bytes() {
        assert_eq!(control_change_message(44, 127), 0x007f_2cb0);
    }

    #[test]
    fn control_change_packing_matches_looper_boundary_bytes() {
        assert_eq!(control_change_message(48, 0), 0x0000_30b0);
        assert_eq!(control_change_message(48, 127), 0x007f_30b0);
        assert_eq!(control_change_message(53, 127), 0x007f_35b0);
        assert_eq!(control_change_message(61, 13), 0x000d_3db0);
    }
}
