use multi_tier_cache::CacheSystem;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .init();

    tracing::info!("Starting tracing verification...");

    // Initialize cache system (should log debug messages)
    let cache = CacheSystem::new().await?;

    // Perform some operations
    let manager = cache.cache_manager();
    
    tracing::info!("Setting value...");
    manager.set_with_strategy("test_key", serde_json::json!("test_value"), multi_tier_cache::CacheStrategy::ShortTerm).await?;

    tracing::info!("Getting value...");
    let value = manager.get("test_key").await?;
    tracing::info!("Got value: {:?}", value);

    Ok(())
}
