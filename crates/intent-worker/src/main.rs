//! `intent-worker <svg|png|jpeg|font>`: one job, then exit (D-011, D-062).
//!
//! Started by `intent-render`'s driver with an empty environment. Before reading a byte it
//! limits itself: CPU time, no file writes, no core dumps, few descriptors and, on Linux, its
//! address space. A limit that cannot be set stops the job (it never runs unconfined). It reads
//! the input from stdin, writes one framed answer to stdout and exits 0; a crash is the
//! driver's to report.

use std::{
    io::{Read as _, Write as _},
    process::ExitCode,
};

use intent_render::{AssetKind, Header, MAX_INPUT_BYTES, WorkerRefusal};
use rlimit::Resource;

/// CPU seconds a job may use (the driver's wall-time deadline is the other bound).
const CPU_SECONDS: u64 = 10;
/// Address space on Linux (1 GiB), where the kernel enforces it.
#[cfg(target_os = "linux")]
const ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;

fn confine() -> std::io::Result<()> {
    rlimit::setrlimit(Resource::CPU, CPU_SECONDS, CPU_SECONDS)?;
    // No file may be written, of any size.
    rlimit::setrlimit(Resource::FSIZE, 0, 0)?;
    rlimit::setrlimit(Resource::CORE, 0, 0)?;
    rlimit::setrlimit(Resource::NOFILE, 16, 16)?;
    #[cfg(target_os = "linux")]
    rlimit::setrlimit(Resource::AS, ADDRESS_SPACE_BYTES, ADDRESS_SPACE_BYTES)?;
    Ok(())
}

fn answer(result: Result<intent_render::Processed, WorkerRefusal>) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match result {
        Ok(processed) => {
            let header = Header {
                ok: true,
                media_type: Some(processed.media_type),
                width: processed.width,
                height: processed.height,
                removed_parts: Some(processed.removed_parts),
                length: Some(processed.output.len()),
                refusal: None,
            };
            serde_json::to_writer(&mut stdout, &header)?;
            stdout.write_all(b"\n")?;
            stdout.write_all(&processed.output)?;
        }
        Err(refusal) => {
            let header = Header {
                ok: false,
                media_type: None,
                width: None,
                height: None,
                removed_parts: None,
                length: None,
                refusal: Some(refusal),
            };
            serde_json::to_writer(&mut stdout, &header)?;
            stdout.write_all(b"\n")?;
        }
    }
    stdout.flush()
}

fn main() -> ExitCode {
    if confine().is_err() {
        return ExitCode::from(3);
    }
    let Some(kind) = std::env::args()
        .nth(1)
        .as_deref()
        .and_then(AssetKind::parse)
    else {
        return ExitCode::from(2);
    };
    let mut input = Vec::new();
    let limit = (MAX_INPUT_BYTES + 1) as u64;
    if std::io::stdin()
        .lock()
        .take(limit)
        .read_to_end(&mut input)
        .is_err()
    {
        return ExitCode::from(4);
    }
    let result = if input.len() > MAX_INPUT_BYTES {
        Err(WorkerRefusal::TooLarge)
    } else {
        intent_worker::process(kind, &input)
    };
    match answer(result) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(5),
    }
}
