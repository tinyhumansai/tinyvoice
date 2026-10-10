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

    let declared: Vec<_> = info
        .manifest
        .provides
        .iter()
        .flat_map(|interface| interface.methods.iter())
        .map(ToString::to_string)
        .collect();
    let mut declared = declared;
    declared.sort_unstable();
    if declared != tinyvoice_bus::METHODS {
        return Err(io::Error::other("capture manifest differs from contract").into());
    }
    // Default-denied permission must reject before any native device is opened.
    let denied: tinyvoice_bus::capture::CaptureResult<tinyvoice_bus::capture::CaptureHandle> =
        proxy
            .call(
                "RecordingStart",
                (tinyvoice_bus::capture::RecordingStartRequest::default(),),
            )
            .await?;
    if denied != Err(tinyvoice_bus::capture::CaptureError::PermissionDenied) {
        return Err(io::Error::other("module did not enforce permission decision").into());
    }

    let denied: tinyvoice_bus::capture::CaptureResult<tinyvoice_bus::capture::CaptureStream> =
        proxy
            .call(
                tinyvoice_bus::names::methods::CAPTURE_START,
                (tinyvoice_bus::capture::RecordingStartRequest::default(),),
            )
            .await?;
    if !matches!(
        denied,
        Err(tinyvoice_bus::capture::CaptureError::PermissionDenied)
    ) {
        return Err(
            std::io::Error::other("continuous capture did not refuse missing permission").into(),
        );
    }

    let denied: tinyvoice_bus::capture::CaptureResult<tinyvoice_bus::capture::CaptureHandle> =
        proxy
            .call(
                tinyvoice_bus::names::methods::RESERVE_CAPTURE,
                (tinyvoice_bus::capture::RecordingStartRequest::default(),),
            )
            .await?;
    if !matches!(
        denied,
        Err(tinyvoice_bus::capture::CaptureError::PermissionDenied)
    ) {
        return Err(io::Error::other("reservation did not refuse missing permission").into());
    }

    verify_hotkeys(&proxy).await?;
    let closed: tinyvoice_bus::capture::CaptureResult<()> = proxy
        .call(tinyvoice_bus::names::methods::CAPTURE_SHUTDOWN, ())
        .await?;
    if closed.is_err() {
        return Err(io::Error::other("capture shutdown failed").into());
    }
    println!(
        "verified {} as TinyBus module `{}`",
        module.display(),
        info.name
    );
    broker_task.abort();
    Ok(())
}

async fn verify_hotkeys(proxy: &tinybus::Proxy) -> Result<(), Box<dyn std::error::Error>> {
    let hotkey: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyHandle> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_RESERVE,
            (host_hotkey_request(),),
        )
        .await?;
    let hotkey =
        hotkey.map_err(|error| io::Error::other(format!("hotkey reserve failed: {error:?}")))?;
    let start_request = tinyvoice_bus::HotkeyHandleRequest {
        handle: hotkey.clone(),
    };
    let started: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_START,
            (start_request.clone(),),
        )
        .await?;
    let started =
        started.map_err(|error| io::Error::other(format!("hotkey start failed: {error:?}")))?;
    let retried: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_START,
            (start_request,),
        )
        .await?;
    if retried.map_err(|error| io::Error::other(format!("hotkey start retry failed: {error:?}")))?
        != started
    {
        return Err(io::Error::other("hotkey start retry changed its generation or state").into());
    }
    let generation = started
        .generation
        .ok_or_else(|| io::Error::other("hotkey start omitted generation"))?;
    let stale: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_FEED,
            (feed_request(
                hotkey.clone(),
                generation.saturating_add(1),
                vec![],
            ),),
        )
        .await?;
    if stale != Err(tinyvoice_bus::HotkeyError::StaleGeneration) {
        return Err(io::Error::other("hotkey feed accepted a stale generation").into());
    }
    verify_hotkey_replay(proxy, hotkey.clone(), generation).await?;
    let stopped: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_STOP,
            (tinyvoice_bus::HotkeyHandleRequest { handle: hotkey },),
        )
        .await?;
    if stopped
        .map_err(|error| io::Error::other(format!("hotkey stop failed: {error:?}")))?
        .state
        != tinyvoice_bus::HotkeyState::Stopped
    {
        return Err(io::Error::other("hotkey stop did not finish the lease").into());
    }
    let closed: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(tinyvoice_bus::names::methods::HOTKEY_SHUTDOWN, ())
        .await?;
    if closed
        .map_err(|error| io::Error::other(format!("hotkey shutdown failed: {error:?}")))?
        .state
        != tinyvoice_bus::HotkeyState::Stopped
    {
        return Err(io::Error::other("hotkey shutdown did not finish").into());
    }
    let after_shutdown: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyHandle> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_RESERVE,
            (host_hotkey_request(),),
        )
        .await?;
    if after_shutdown != Err(tinyvoice_bus::HotkeyError::Closed) {
        return Err(io::Error::other("hotkey shutdown left admission open").into());
    }
    Ok(())
}

async fn verify_hotkey_replay(
    proxy: &tinybus::Proxy,
    handle: tinyvoice_bus::HotkeyHandle,
    generation: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let facts = vec![
        tinyvoice_bus::SequencedHostFact {
            sequence: 1,
            fact: tinyvoice_bus::HostKeyFact::Down,
        },
        tinyvoice_bus::SequencedHostFact {
            sequence: 2,
            fact: tinyvoice_bus::HostKeyFact::Up,
        },
    ];
    let fed: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_FEED,
            (feed_request(handle.clone(), generation, facts),),
        )
        .await?;
    fed.map_err(|error| io::Error::other(format!("hotkey feed failed: {error:?}")))?;
    let read_request = tinyvoice_bus::HotkeyReadRequest {
        handle: handle.clone(),
        acknowledged_batch: None,
    };
    let first: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyBatch> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_READ,
            (read_request.clone(),),
        )
        .await?;
    let first =
        first.map_err(|error| io::Error::other(format!("hotkey read failed: {error:?}")))?;
    let replay: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyBatch> = proxy
        .call(tinyvoice_bus::names::methods::HOTKEY_READ, (read_request,))
        .await?;
    if replay.map_err(|error| io::Error::other(format!("hotkey replay failed: {error:?}")))?
        != first
        || first
            .events
            .iter()
            .map(|event| event.event)
            .collect::<Vec<_>>()
            != [
                tinyvoice_bus::HotkeyEvent::Pressed,
                tinyvoice_bus::HotkeyEvent::Released,
            ]
    {
        return Err(
            io::Error::other("hotkey read did not replay the ordered activation batch").into(),
        );
    }
    let ack: tinyvoice_bus::HotkeyResult<tinyvoice_bus::HotkeyBatch> = proxy
        .call(
            tinyvoice_bus::names::methods::HOTKEY_READ,
            (tinyvoice_bus::HotkeyReadRequest {
                handle,
                acknowledged_batch: Some(first.batch),
            },),
        )
        .await?;
    let ack =
        ack.map_err(|error| io::Error::other(format!("hotkey acknowledgment failed: {error:?}")))?;
    if !ack.events.is_empty() || ack.active {
        return Err(io::Error::other(
            "hotkey acknowledgment did not advance to the inactive snapshot",
        )
        .into());
    }
    Ok(())
}

fn host_hotkey_request() -> tinyvoice_bus::HotkeyReserveRequest {
    tinyvoice_bus::HotkeyReserveRequest {
        request: tinyvoice_bus::HotkeyRequest {
            key: "Fn".to_owned(),
            mode: tinyvoice_bus::ActivationMode::Push,
            source: tinyvoice_bus::HotkeySource::Host,
        },
    }
}

fn feed_request(
    handle: tinyvoice_bus::HotkeyHandle,
    generation: u64,
    facts: Vec<tinyvoice_bus::SequencedHostFact>,
) -> tinyvoice_bus::HotkeyFeedRequest {
    tinyvoice_bus::HotkeyFeedRequest {
        handle,
        generation,
        facts,
        overflow: false,
    }
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
