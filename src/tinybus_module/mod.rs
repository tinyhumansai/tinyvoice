//! `TinyBus` module entrypoint and bus-facing interface.
//!
//! This adapter keeps the feature implementation independent from `TinyBus` while
//! exposing it as an installable, dynamically loaded integration.

use tinybus::{Connection, Result as TinyBusResult};

const INTERFACE: &str = "ai.tinyhumans.rust_template.Greeting";
const OBJECT_PATH: &str = "/ai/tinyhumans/rust_template/Greeting";

struct GreetingService;

#[tinybus::interface(name = "ai.tinyhumans.rust_template.Greeting")]
impl GreetingService {
    async fn greet(&self, name: String) -> TinyBusResult<String> {
        std::future::ready(crate::greet(&name))
            .await
            .map_err(|error| tinybus::Error::failed(error.to_string()))
    }
}

async fn setup(connection: Connection) -> TinyBusResult<()> {
    connection
        .serve_at(OBJECT_PATH.try_into()?, GreetingService)
        .await?;
    connection.request_name(INTERFACE).await?;
    Ok(())
}

tinybus_module::module_export! {
    setup = setup,
    worker_threads = 1,
    provides = ["ai.tinyhumans.rust_template.Greeting"],
    methods = ["Greet"],
    signals = [],
    requires = [],
    optional = [],
    lazy = false,
}

#[cfg(test)]
mod test;
