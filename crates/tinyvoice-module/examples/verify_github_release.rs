//! Downloads a tagged release asset and calls the loaded `TinyBus` module.
//!
//! Run it with the release tag URL, platform archive, and archive SHA-256:
//!
//! ```text
//! cargo run --example verify_github_release -- \
//!   https://github.com/tinyhumansai/tinyvoice/releases/tag/v0.1.4 \
//!   tinyvoice-0.1.4-ubuntu-24.04-x86_64.tar.gz \
//!   <sha256>
//! ```

use std::io;
use std::time::Duration;

use tinybus::Connection;
use tinybus::broker::Broker;
use tinybus::module::ModuleHost;
use tinybus::transport::memory::MemoryBus;

// Imported, never redeclared. A local copy of these is exactly how this
// example once ended up dialling `/ai.tinyhumans.tinyvoice.Voice` — a bus name
// where an object path belongs — while the module served the correct path and
// every in-process test still passed.
use tinyvoice_module::{BUS_NAME, OBJECT_PATH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (release_url, archive, sha256) = arguments()?;
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.spawn(bus.clone());
    let module_host = ModuleHost::new(broker);
    let info = module_host.load_github_release(
        &release_url,
        &archive,
        Some(&sha256),
        serde_json::Value::default(),
    )?;

    if info.name != env!("CARGO_PKG_NAME") {
        return Err(io::Error::other(format!(
            "loaded module `{}` instead of `{}`",
            info.name,
            env!("CARGO_PKG_NAME")
        ))
        .into());
    }

    let client = Connection::connect(bus.connect().await?).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let names = client.list_names().await?;
            if names.iter().any(|name| name.as_str() == BUS_NAME) {
                return tinybus::Result::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;

    let proxy = client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)?;
    // A round trip that exercises real logic rather than an echo: the wake
    // word must be matched fuzzily and stripped, and the remainder classified.
    let intent: String = proxy.call("Route", ("please pause the music",)).await?;
    if !intent.contains("\"pause\"") {
        return Err(
            io::Error::other(format!("module returned an unexpected intent: {intent}")).into(),
        );
    }

    println!(
        "verified {archive} from {release_url} as TinyBus module `{}`",
        info.name
    );
    broker_task.abort();
    Ok(())
}

fn arguments() -> Result<(String, String, String), io::Error> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: cargo run --example verify_github_release -- \
                 <release-tag-url> <archive-name> <sha256>";
    let release_url = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let archive = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let sha256 = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    if args.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, usage));
    }
    Ok((release_url, archive, sha256))
}
