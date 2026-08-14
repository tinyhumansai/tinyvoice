//! Loads a built module through the real `TinyBus` dynamic loader.

use std::io;
use std::path::PathBuf;
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
    let module = module_argument()?;
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.spawn(bus.clone());
    let module_host = ModuleHost::new(broker);
    let info = module_host.load_file(&module)?;

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
        "verified {} as TinyBus module `{}`",
        module.display(),
        info.name
    );
    broker_task.abort();
    Ok(())
}

fn module_argument() -> Result<PathBuf, io::Error> {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: cargo run --example verify_module -- <module-path>",
            )
        })
}
