//! Measure the real cost of a module call, so the VAD's placement is decided
//! by a number rather than an assumption.
//!
//! The question this answers: an always-on capture loop drives a segmenter once
//! per audio frame — every 20 ms. Is a bus round trip cheap enough to sit on
//! that path, or does the hop cost more than the work?
//!
//! Run against a built module:
//!
//! ```sh
//! cargo run --release -p tinyvoice-module --example bench_call -- \
//!   target/release/libtinyvoice_module.so
//! ```

// FRAMES is a small literal; the cast is exact.
#![allow(clippy::cast_possible_truncation)]

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tinybus::Connection;
use tinybus::broker::Broker;
use tinybus::module::ModuleHost;
use tinybus::transport::memory::MemoryBus;
use tinyvoice_module::{BUS_NAME, OBJECT_PATH};

/// One 20 ms frame at 16 kHz, as the always-on loop would deliver it.
const FRAME_MS: u32 = 20;

/// How many frames to push per measurement.
const FRAMES: usize = 500;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: cargo run --example bench_call -- <module-path>",
            )
        })?;

    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.spawn(bus.clone());
    let module_host = ModuleHost::new(broker);
    module_host.load_file(&module)?;

    let client = Connection::connect(bus.connect().await?).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if client
                .list_names()
                .await?
                .iter()
                .any(|n| n.as_str() == BUS_NAME)
            {
                return tinybus::Result::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;

    let proxy = client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default())?;

    // Warm up: the first call pays for connection setup and any lazy init.
    for _ in 0..50 {
        let _: String = proxy
            .call("Segment", (config.clone(), FRAME_MS, vec![0.5f32]))
            .await?;
    }

    // (a) One call per frame — what a naive always-on loop would do.
    let started = Instant::now();
    for i in 0..FRAMES {
        let energy = if i % 3 == 0 { 0.5f32 } else { 0.0f32 };
        let _: String = proxy
            .call("Segment", (config.clone(), FRAME_MS, vec![energy]))
            .await?;
    }
    let per_frame = started.elapsed();

    // (b) One call for the whole run — the batch shape the interface offers.
    let energies: Vec<f32> = (0..FRAMES)
        .map(|i| if i % 3 == 0 { 0.5f32 } else { 0.0f32 })
        .collect();
    let started = Instant::now();
    let _: String = proxy.call("Segment", (config, FRAME_MS, energies)).await?;
    let batched = started.elapsed();

    let per_call = per_frame / FRAMES as u32;
    let frame_budget = Duration::from_millis(u64::from(FRAME_MS));

    println!("frames                : {FRAMES} @ {FRAME_MS}ms");
    println!("per-frame calls       : {per_frame:?} total, {per_call:?} per call");
    println!("one batched call      : {batched:?}");
    println!("frame budget          : {frame_budget:?}");
    println!(
        "per-call / budget     : {:.4}%",
        per_call.as_secs_f64() / frame_budget.as_secs_f64() * 100.0
    );

    broker_task.abort();
    Ok(())
}
