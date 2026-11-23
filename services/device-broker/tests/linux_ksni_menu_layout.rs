#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ksni::menu::{StandardItem, SubMenu};
use ksni::qcr_test_support::Layout as KsniLayout;
use ksni::TrayMethods as _;
use zbus::{connection::Builder, Connection, OwnedVariant, Proxy};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const MENU_PATH: &str = "/MenuBar";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

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
    properties: &std::collections::HashMap<std::borrow::Cow<'static, str>, zbus::OwnedVariant>,
    key: &str,
) -> String {
    String::try_from(properties[key].clone()).expect("menu string property")
}

fn property_bool(
    properties: &std::collections::HashMap<std::borrow::Cow<'static, str>, zbus::OwnedVariant>,
    key: &str,
) -> bool {
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
        .build()
        .await
        .expect("watcher service starts");

    let tray = TestTray.spawn().await.expect("KSNI tray starts");
    let response = async {
        let item_name = wait_for_item(&watcher).await?;
        let client = Connection::session().await?;
        let proxy = Proxy::new(&client, item_name, MENU_PATH, MENU_INTERFACE).await?;
        let (_revision, root): (u32, OwnedVariant) = proxy
            .call("GetLayout", &(0_i32, -1_i32, Vec::<String>::new()))
            .await?;
        let root = KsniLayout::try_from(root)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(root)
    }
    .await;
    tray.shutdown().await;
    let root = response.expect("KSNI tray registers and returns its D-Bus menu layout");

    assert_eq!(root.id, 0);
    assert_eq!(root.children.len(), 2);
    let first = KsniLayout::try_from(
        root.children[0]
            .clone()
            .try_into_owned()
            .expect("first layout child is owned"),
    )
    .expect("first layout child uses KSNI's layout decoder");
    assert_eq!(
        property_string(&first.properties, "label"),
        "Open QC Remote"
    );
    assert!(property_bool(&first.properties, "enabled"));
    assert!(property_bool(&first.properties, "visible"));

    let submenu = KsniLayout::try_from(
        root.children[1]
            .clone()
            .try_into_owned()
            .expect("submenu layout child is owned"),
    )
    .expect("submenu layout child uses KSNI's layout decoder");
    assert_eq!(property_string(&submenu.properties, "label"), "Connection");
    assert_eq!(submenu.children.len(), 1);
    let nested = KsniLayout::try_from(
        submenu.children[0]
            .clone()
            .try_into_owned()
            .expect("nested layout child is owned"),
    )
    .expect("nested layout child uses KSNI's layout decoder");
    assert_eq!(property_string(&nested.properties, "label"), "Reconnect");
    assert!(!property_bool(&nested.properties, "enabled"));
}
