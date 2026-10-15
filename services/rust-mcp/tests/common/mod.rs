//! Exact fixture task ownership. Drop schedules abort; it does not synchronously acknowledge task retirement.
pub struct OwnedTask<T>(pub tokio::task::JoinHandle<T>);
impl<T> Drop for OwnedTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl<T> OwnedTask<T> {
    pub async fn join(mut self) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(2), &mut self.0)
            .await
            .expect("fixture server did not stop within its join budget")
            .expect("fixture task failed")
    }
}
