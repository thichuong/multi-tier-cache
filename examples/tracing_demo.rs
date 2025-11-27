use multi_tier_cache::CacheSystem;
use tracing_subscriber::{fmt, EnvFilter}; // Import fmt module

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    // Sử dụng fmt::Subscriber builder thay vì gọi thẳng fmt() đôi khi rõ ràng hơn
    // với compiler trong các trường hợp macro phức tạp
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .init();

    tracing::info!("Starting tracing verification...");

    // Initialize cache system
    let cache = CacheSystem::new().await?;

    // Perform a simple operation
    let manager = cache.cache_manager();
    manager
        .set_with_strategy(
            "test_key",
            serde_json::json!("value"),
            multi_tier_cache::CacheStrategy::ShortTerm,
        )
        .await?;

    tracing::info!("Operation complete");
    Ok(())
}
