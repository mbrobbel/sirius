//! Construct a public engine context from YAML or built-in defaults.

use sirius::{Context, ContextConfigBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let builder = match std::env::args_os().nth(1) {
        Some(path) => ContextConfigBuilder::from_yaml(path)?,
        None => ContextConfigBuilder::new()?,
    };
    let config = builder.build()?;
    let context = Context::new(&config)?;
    drop(config);
    println!("Sirius engine initialized successfully.");
    drop(context);
    Ok(())
}
