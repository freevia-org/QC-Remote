//! Performance MIDI is an explicit native capability. Linux HID never silently
//! substitutes for a MIDI command or chooses an unrelated output by name.
#[cfg(windows)]
pub(crate) use qc_windows_midi::{PerformanceMidi, PerformanceMidiReceipt};

#[cfg(not(windows))]
pub(crate) use unavailable::{PerformanceMidi, PerformanceMidiReceipt};

#[cfg(not(windows))]
mod unavailable {
    const UNAVAILABLE: &str = "Performance MIDI is not available in this headless Linux milestone; a verified HID-to-ALSA device mapping is required";

    #[derive(Default)]
    pub struct PerformanceMidi {
        _private: (),
    }

    pub struct PerformanceMidiReceipt {
        pub endpoint: String,
        pub dispatch_latency_ms: f64,
        pub throttle_delay_ms: f64,
    }

    impl PerformanceMidi {
        pub fn send_for_device(
            &mut self,
            _device_id: &str,
            _controller: u8,
            _value: u8,
        ) -> Result<PerformanceMidiReceipt, String> {
            Err(UNAVAILABLE.into())
        }

        #[cfg(test)]
        pub fn warm_up_for_device(&mut self, _device_id: &str) -> Result<String, String> {
            Err(UNAVAILABLE.into())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unsupported_midi_never_reports_delivery() {
            let mut midi = PerformanceMidi::default();
            assert_eq!(midi.warm_up_for_device("device").unwrap_err(), UNAVAILABLE);
            assert!(
                matches!(midi.send_for_device("device", 0, 0), Err(error) if error == UNAVAILABLE)
            );
        }
    }
}
