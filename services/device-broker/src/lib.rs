pub mod cli;
pub mod control;
mod demo_provider;
pub mod device_lease;
mod generated_broker_surface;
pub mod orphan_guard;
pub mod platform;
pub mod resident;
pub mod rpc;
pub mod target;
pub mod tray;
pub mod tray_settings;
pub mod updater_relay;
pub mod user_quit;
pub mod worker;
pub mod write_quarantine;

#[cfg(test)]
mod descriptor_debug_tests {
    use crate::generated_broker_surface::BrokerDescriptor;

    #[test]
    fn a_formatted_descriptor_never_shows_the_bearer_token() {
        let descriptor = BrokerDescriptor {
            port: 54123,
            bearer_token: "s3cret-broker-token".into(),
            instance_id: "live-broker".into(),
            process_id: 42,
            process_created_at: None,
            published_at_unix_ms: 1,
            gateway_api_version: 3,
        };
        let shown = format!("{descriptor:?}");
        assert!(!shown.contains("s3cret-broker-token"), "{shown}");
        assert!(shown.contains("<redacted>") && shown.contains("live-broker"));
        assert!(serde_json::to_string(&descriptor)
            .unwrap()
            .contains("s3cret-broker-token"));
    }
}
