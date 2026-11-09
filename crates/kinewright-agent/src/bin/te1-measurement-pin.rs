//! Prints unpaid measurement observations. Never launches a provider.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&kinewright_agent::te1_eval::pin::record()?)?
    );
    Ok(())
}
