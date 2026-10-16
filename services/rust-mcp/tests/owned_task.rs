mod common;

#[tokio::test]
async fn owned_fixture_task_unwind_aborts_the_exact_task_and_acknowledges_local_drop() {
    struct DropAck(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for DropAck {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    let (ack, retired) = tokio::sync::oneshot::channel();
    let (entered, running) = tokio::sync::oneshot::channel();
    let task = common::OwnedTask(tokio::spawn(async move {
        let _drop_ack = DropAck(Some(ack));
        let _ = entered.send(());
        std::future::pending::<()>().await;
    }));
    tokio::time::timeout(std::time::Duration::from_secs(1), running)
        .await
        .unwrap()
        .unwrap();
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _task = task;
        panic!("controlled fixture unwind");
    }));
    assert!(unwind.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(1), retired)
        .await
        .expect("the exact canceled task did not acknowledge local destruction")
        .expect("task-local Drop did not send its acknowledgement");
}

#[tokio::test]
async fn owned_fixture_task_healthy_join_acknowledges_its_original_result() {
    assert_eq!(
        common::OwnedTask(tokio::spawn(async { 7_u8 })).join().await,
        7
    );
}
