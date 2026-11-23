#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ksni::menu::{StandardItem, SubMenu};
use ksni::TrayMethods as _;
use zbus::{connection::Builder, Connection, OwnedVariant, Proxy};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const MENU_PATH: &str = "/MenuBar";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

type LayoutTuple = (
    i32,
    std::collections::HashMap<String, OwnedVariant>,
    Vec<OwnedVariant>,
);

#[derive(Clone, Default)]
struct TestWatcher {
    registered_items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl TestWatcher {
    async fn register_status_notifier_item(&self, service: &str) -> zbus::fdo::Result<()> {
        self.registered_items
            .lock()
            .expect("watcher registrations lock")
            .push(service.to_owned());
        Ok(())
    }

    async fn register_status_notifier_host(&self, _service: &str) -> zbus::fdo::Result<()> {
        Ok(())
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::fdo::Result<Vec<String>> {
        Ok(self
            .registered_items
            .lock()
            .expect("watcher registrations lock")
            .clone())
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> zbus::fdo::Result<bool> {
        Ok(true)
    }

    #[zbus(property)]
    fn protocol_version(&self) -> zbus::fdo::Result<i32> {
        Ok(0)
    }
}

struct TestTray;

impl ksni::Tray for TestTray {
    fn id(&self) -> String {
        "qcr-ksni-layout-conversion-test".into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open QC Remote".into(),
                enabled: true,
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Connection".into(),
                submenu: vec![StandardItem {
                    label: "Reconnect".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into()],
                ..Default::default()
            }
            .into(),
        ]
    }
}

async fn wait_for_item(watcher: &TestWatcher) -> Result<String, tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(item) = watcher
                .registered_items
                .lock()
                .expect("watcher registrations lock")
                .last()
                .cloned()
            {
                return item;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

fn property_string(
    properties: &std::collections::HashMap<String, OwnedVariant>,
    key: &str,
) -> String {
    String::try_from(properties[key].clone()).expect("menu string property")
}

fn property_bool(properties: &std::collections::HashMap<String, OwnedVariant>, key: &str) -> bool {
    bool::try_from(properties[key].clone()).expect("menu boolean property")
}

#[test]
fn ksni_menu_layout_round_trips_string_keys_and_variant_properties_on_zbus6() {
    zbus::block_on(assert_menu_layout());
}

async fn assert_menu_layout() {
    let watcher = TestWatcher::default();
    let _watcher_connection = Builder::session()
        .name(WATCHER_NAME)
        .serve_at(WATCHER_PATH, watcher.clone())
        .expect("watcher object registers")
        .build()
        .await
        .expect("watcher service starts");

    let tray = TestTray.spawn().await.expect("KSNI tray starts");
    let response = async {
        let item_name = wait_for_item(&watcher).await?;
        let client = Connection::session().await?;
        let proxy = Proxy::new(&client, item_name, MENU_PATH, MENU_INTERFACE).await?;
        let (_revision, root): (u32, LayoutTuple) = proxy
            .call("GetLayout", &(0_i32, -1_i32, Vec::<String>::new()))
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(root)
    }
    .await;
    tray.shutdown().await;
    let root = response.expect("KSNI tray registers and returns its D-Bus menu layout");

    assert_eq!(root.0, 0);
    assert_eq!(root.2.len(), 2);
    let first: LayoutTuple = root.2[0]
        .clone()
        .try_into()
        .expect("first layout child decodes");
    assert_eq!(property_string(&first.1, "label"), "Open QC Remote");
    assert!(property_bool(&first.1, "enabled"));
    assert!(property_bool(&first.1, "visible"));

    let submenu: LayoutTuple = root.2[1]
        .clone()
        .try_into()
        .expect("submenu layout child decodes");
    assert_eq!(property_string(&submenu.1, "label"), "Connection");
    assert_eq!(submenu.2.len(), 1);
    let nested: LayoutTuple = submenu.2[0]
        .clone()
        .try_into()
        .expect("nested layout child decodes");
    assert_eq!(property_string(&nested.1, "label"), "Reconnect");
    assert!(!property_bool(&nested.1, "enabled"));
}
