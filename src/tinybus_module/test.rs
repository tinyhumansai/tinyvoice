//! Tests for the `TinyBus` module adapter and its declared surface.

use super::{GreetingService, INTERFACE, OBJECT_PATH, setup};
use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;
use tinybus::{Connection, Interface};

#[test]
fn declared_methods_match_the_dispatch_table() {
    let methods = GreetingService
        .members()
        .into_iter()
        .map(|member| member.to_string())
        .collect::<Vec<_>>();

    assert_eq!(methods, ["Greet"]);
}

#[tokio::test]
async fn module_serves_greetings_over_a_real_bus() -> tinybus::Result<()> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone()).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(INTERFACE, OBJECT_PATH, INTERFACE)?;
    let greeting: String = proxy.call("Greet", ("Ferris",)).await?;

    assert_eq!(greeting, "Hello, Ferris!");
    Ok(())
}

#[tokio::test]
async fn module_rejects_an_empty_name_over_the_bus() -> tinybus::Result<()> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone()).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(INTERFACE, OBJECT_PATH, INTERFACE)?;
    let result = proxy.call::<String>("Greet", ("   ",)).await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "whitespace-only names unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("name must not be empty"));
    Ok(())
}
