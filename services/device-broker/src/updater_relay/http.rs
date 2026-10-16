pub use quadcortex_forwarder::http::DirectHttpFetcher;
pub(crate) fn direct_fetcher() -> DirectHttpFetcher {
    DirectHttpFetcher::with_resolver(std::sync::Arc::new(|host| {
        Box::pin(async move { super::resolver_helper::resolve(&host).await })
    }))
}
