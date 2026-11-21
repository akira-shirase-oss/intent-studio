//! The driver for the isolated asset worker (D-011, D-062).
//!
//! Untrusted bytes — an uploaded SVG, image or font — are never decoded in the API process. Each
//! job is one `intent-worker` process: no environment (so no database URL, token or key reaches
//! it), stdin carries the input, stdout carries one framed answer, stderr is discarded. The
//! driver bounds the job by wall time, by output size and by how many run at once, and maps
//! every way a job can end — an answer, a refusal, a crash by signal, a hang, garbage on stdout
//! — to a [`Processed`] or a named [`WorkerRefusal`]. A worker that dies takes only its own job
//! with it.
//!
//! The protocol (version 1): the worker is started as `intent-worker <kind>`; it reads all of
//! stdin, then writes one JSON header line — `{"ok":true,"media_type":...,"width":...,
//! "height":...,"removed_parts":...,"length":N}` followed by exactly N output bytes, or
//! `{"ok":false,"refusal":"<code>"}` — and exits 0.

use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    sync::Semaphore,
};

/// Largest input a job takes (10 MiB).
pub const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
/// Largest output a job may produce (a re-encoded image can outgrow its input).
pub const MAX_OUTPUT_BYTES: usize = 24 * 1024 * 1024;
/// Longest header line.
pub const MAX_HEADER_BYTES: usize = 1024;
/// Default wall time for one job.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);
/// Default jobs at once (each may use several hundred MiB while decoding).
pub const DEFAULT_CONCURRENCY: usize = 2;

/// What kind of file a job checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    /// SVG: sanitized to an element and attribute allowlist.
    Svg,
    /// PNG: decoded and re-encoded.
    Png,
    /// JPEG: decoded and re-encoded.
    Jpeg,
    /// TrueType or OpenType font: parsed and checked.
    Font,
    /// A site bundle: the static half of the check suite (D-022), answering a
    /// JSON report instead of transformed bytes.
    Check,
}

impl AssetKind {
    /// The worker's argument.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Font => "font",
            Self::Check => "check",
        }
    }

    /// Parses the worker's argument.
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Svg, Self::Png, Self::Jpeg, Self::Font, Self::Check]
            .into_iter()
            .find(|k| k.as_str() == text)
    }

    /// The kind an upload's media type asks for; `None` for anything not accepted.
    pub fn from_media_type(media_type: &str) -> Option<Self> {
        let essence = media_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match essence.as_str() {
            "image/svg+xml" => Some(Self::Svg),
            "image/png" => Some(Self::Png),
            "image/jpeg" | "image/jpg" => Some(Self::Jpeg),
            "font/ttf"
            | "font/otf"
            | "font/sfnt"
            | "application/font-sfnt"
            | "application/x-font-ttf"
            | "application/x-font-otf"
            | "application/vnd.ms-opentype" => Some(Self::Font),
            _ => None,
        }
    }
}

/// Why a job produced nothing. Stable codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRefusal {
    /// The type is not one this build accepts (WebP, WOFF2, GIF...).
    #[error("unsupported_type")]
    UnsupportedType,
    /// The bytes are not the type the upload says.
    #[error("type_mismatch")]
    TypeMismatch,
    /// The bytes do not decode or parse as their type.
    #[error("malformed")]
    Malformed,
    /// Larger than the input cap, the output cap, or the pixel limit.
    #[error("too_large")]
    TooLarge,
    /// The job ran past its deadline and was killed.
    #[error("worker_timeout")]
    WorkerTimeout,
    /// The worker died (a signal, an abort, a non-zero exit) or answered garbage.
    #[error("worker_crashed")]
    WorkerCrashed,
    /// The worker could not be started.
    #[error("worker_unavailable")]
    WorkerUnavailable,
    /// Every job slot stayed busy for the whole wait.
    #[error("worker_busy")]
    WorkerBusy,
}

impl WorkerRefusal {
    /// The wire code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedType => "unsupported_type",
            Self::TypeMismatch => "type_mismatch",
            Self::Malformed => "malformed",
            Self::TooLarge => "too_large",
            Self::WorkerTimeout => "worker_timeout",
            Self::WorkerCrashed => "worker_crashed",
            Self::WorkerUnavailable => "worker_unavailable",
            Self::WorkerBusy => "worker_busy",
        }
    }
}

/// The worker's header line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// Whether the job produced output.
    pub ok: bool,
    /// The output's media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Pixel width, for images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Pixel height, for images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Elements and attributes the sanitizer dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_parts: Option<u32>,
    /// The names of what was refused (`element` or `element@attribute`), bounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_parts: Option<Vec<String>>,
    /// Output bytes that follow the header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    /// Why there is no output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<WorkerRefusal>,
}

/// A job's result.
#[derive(Clone, PartialEq, Eq)]
pub struct Processed {
    /// `image/svg+xml`, `image/png`, `image/jpeg`, `font/ttf` or `font/otf`.
    pub media_type: String,
    /// Pixel width, for images.
    pub width: Option<u32>,
    /// Pixel height, for images.
    pub height: Option<u32>,
    /// What the sanitizer dropped.
    pub removed_parts: u32,
    /// The names of the first refused items, for reporting (bounded by the sanitizer).
    pub dropped_parts: Vec<String>,
    /// The bytes to store and serve.
    pub output: Vec<u8>,
}

impl std::fmt::Debug for Processed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processed")
            .field("media_type", &self.media_type)
            .field("bytes", &self.output.len())
            .finish_non_exhaustive()
    }
}

/// Splits the worker's stdout into a [`Processed`] or a refusal; anything malformed is a crash.
pub fn decode_answer(stdout: &[u8]) -> Result<Processed, WorkerRefusal> {
    let Some(newline) = stdout
        .iter()
        .take(MAX_HEADER_BYTES)
        .position(|b| *b == b'\n')
    else {
        return Err(WorkerRefusal::WorkerCrashed);
    };
    let header: Header =
        serde_json::from_slice(&stdout[..newline]).map_err(|_| WorkerRefusal::WorkerCrashed)?;
    let body = &stdout[newline + 1..];
    if !header.ok {
        return Err(header.refusal.unwrap_or(WorkerRefusal::WorkerCrashed));
    }
    let allowed = [
        "image/svg+xml",
        "image/png",
        "image/jpeg",
        "font/ttf",
        "font/otf",
        "application/json",
    ];
    match (header.media_type, header.length) {
        (Some(media_type), Some(length))
            if length == body.len() && length > 0 && allowed.contains(&media_type.as_str()) =>
        {
            Ok(Processed {
                media_type,
                width: header.width,
                height: header.height,
                removed_parts: header.removed_parts.unwrap_or(0),
                dropped_parts: header.dropped_parts.unwrap_or_default(),
                output: body.to_vec(),
            })
        }
        _ => Err(WorkerRefusal::WorkerCrashed),
    }
}

/// Runs jobs, each in a fresh worker process.
#[derive(Debug, Clone)]
pub struct Driver {
    program: PathBuf,
    args: Vec<String>,
    deadline: Duration,
    slots: Arc<Semaphore>,
    slot_wait: Duration,
}

impl Driver {
    /// A driver for the worker at `program`.
    pub fn new(program: PathBuf, deadline: Duration, concurrency: usize) -> Self {
        Self {
            program,
            args: Vec::new(),
            deadline,
            slots: Arc::new(Semaphore::new(concurrency.max(1))),
            slot_wait: Duration::from_secs(5),
        }
    }

    /// Arguments placed before the kind (for tests that stand a script in for the worker).
    #[must_use]
    pub fn with_leading_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Checks `input` as `kind` in a fresh worker.
    pub async fn run(&self, kind: AssetKind, input: &[u8]) -> Result<Processed, WorkerRefusal> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(WorkerRefusal::TooLarge);
        }
        let _slot = tokio::time::timeout(self.slot_wait, self.slots.clone().acquire_owned())
            .await
            .map_err(|_| WorkerRefusal::WorkerBusy)?
            .map_err(|_| WorkerRefusal::WorkerUnavailable)?;
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .arg(kind.as_str())
            // No credentials, no configuration, no proxy settings: nothing from the server's
            // environment reaches the worker.
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| WorkerRefusal::WorkerUnavailable)?;
        let mut stdin = child.stdin.take().ok_or(WorkerRefusal::WorkerUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(WorkerRefusal::WorkerUnavailable)?;
        let job = async {
            // Write and read at the same time: a worker that answers early (or never reads)
            // must not deadlock against a full pipe.
            let write = async {
                // A worker that exits without reading closes the pipe; that is its answer's
                // business, not an error here.
                let _ = stdin.write_all(input).await;
                drop(stdin);
            };
            let read = async {
                let mut out = Vec::new();
                let limit = (MAX_OUTPUT_BYTES + MAX_HEADER_BYTES + 1) as u64;
                stdout.take(limit).read_to_end(&mut out).await.map(|_| out)
            };
            let ((), out) = tokio::join!(write, read);
            let out = out.map_err(|_| WorkerRefusal::WorkerCrashed)?;
            if out.len() > MAX_OUTPUT_BYTES + MAX_HEADER_BYTES {
                let _ = child.start_kill();
                return Err(WorkerRefusal::TooLarge);
            }
            match child.wait().await {
                Ok(status) if status.success() => decode_answer(&out),
                // Killed by a signal, aborted, or exited non-zero: whatever it printed is not
                // trusted.
                _ => Err(WorkerRefusal::WorkerCrashed),
            }
        };
        // Past the deadline, `kill_on_drop` ends the process when `child` goes out of scope.
        tokio::time::timeout(self.deadline, job)
            .await
            .unwrap_or(Err(WorkerRefusal::WorkerTimeout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(header: &Header, body: &[u8]) -> Vec<u8> {
        let mut out = serde_json::to_vec(header).unwrap();
        out.push(b'\n');
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn answers_decode_and_garbage_is_a_crash() {
        let ok = Header {
            ok: true,
            media_type: Some("image/png".into()),
            width: Some(2),
            height: Some(3),
            removed_parts: None,
            dropped_parts: None,
            length: Some(4),
            refusal: None,
        };
        let processed = decode_answer(&frame(&ok, b"abcd")).unwrap();
        assert_eq!((processed.width, processed.output.len()), (Some(2), 4));
        // A length that does not match, an unknown media type, a missing newline, junk.
        assert_eq!(
            decode_answer(&frame(&ok, b"abc")),
            Err(WorkerRefusal::WorkerCrashed)
        );
        let mut html = ok.clone();
        html.media_type = Some("text/html".into());
        assert_eq!(
            decode_answer(&frame(&html, b"abcd")),
            Err(WorkerRefusal::WorkerCrashed)
        );
        assert_eq!(
            decode_answer(b"{\"ok\":true"),
            Err(WorkerRefusal::WorkerCrashed)
        );
        assert_eq!(
            decode_answer(b"Segmentation fault\n"),
            Err(WorkerRefusal::WorkerCrashed)
        );
        let refused = Header {
            ok: false,
            media_type: None,
            width: None,
            height: None,
            removed_parts: None,
            dropped_parts: None,
            length: None,
            refusal: Some(WorkerRefusal::Malformed),
        };
        assert_eq!(
            decode_answer(&frame(&refused, b"")),
            Err(WorkerRefusal::Malformed)
        );
    }

    #[test]
    fn media_types_map_to_kinds() {
        assert_eq!(
            AssetKind::from_media_type("image/svg+xml; charset=utf-8"),
            Some(AssetKind::Svg)
        );
        assert_eq!(
            AssetKind::from_media_type("IMAGE/PNG"),
            Some(AssetKind::Png)
        );
        assert_eq!(
            AssetKind::from_media_type("font/otf"),
            Some(AssetKind::Font)
        );
        assert_eq!(AssetKind::from_media_type("image/webp"), None);
        assert_eq!(AssetKind::from_media_type("text/html"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_crashing_hanging_or_absent_worker_fails_only_its_job() {
        let sh =
            |script: &str| {
                Driver::new("/bin/sh".into(), Duration::from_millis(500), 2)
                    .with_leading_args(vec!["-c".into(), script.into(), "worker".into()])
            };
        // Killed by a signal mid-job.
        assert_eq!(
            sh("kill -SEGV $$").run(AssetKind::Png, b"x").await,
            Err(WorkerRefusal::WorkerCrashed)
        );
        // A valid-looking answer, then a non-zero exit: not trusted.
        assert_eq!(
            sh("printf '{\"ok\":true,\"media_type\":\"image/png\",\"length\":1}\\nx'; exit 3")
                .run(AssetKind::Png, b"x")
                .await,
            Err(WorkerRefusal::WorkerCrashed)
        );
        // Hangs past the deadline: killed.
        let started = std::time::Instant::now();
        assert_eq!(
            sh("sleep 30").run(AssetKind::Png, b"x").await,
            Err(WorkerRefusal::WorkerTimeout)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        // Floods stdout: cut off at the cap and killed.
        let flood = Driver::new("/bin/sh".into(), Duration::from_secs(20), 1)
            .with_leading_args(vec!["-c".into(), "yes".into(), "worker".into()]);
        assert_eq!(
            flood.run(AssetKind::Png, b"x").await,
            Err(WorkerRefusal::TooLarge)
        );
        // Cannot be started at all.
        let missing = Driver::new(
            "/nonexistent/intent-worker".into(),
            Duration::from_secs(1),
            1,
        );
        assert_eq!(
            missing.run(AssetKind::Png, b"x").await,
            Err(WorkerRefusal::WorkerUnavailable)
        );
        // The environment is empty: nothing the server's process holds reaches a job. Cargo
        // sets `CARGO_MANIFEST_DIR` and `CARGO_PKG_NAME` for this test process; the job must
        // see neither.
        assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
        let env = sh(
            "if [ -z \"$CARGO_MANIFEST_DIR$CARGO_PKG_NAME$USER\" ]; then printf '{\"ok\":false,\"refusal\":\"malformed\"}\\n'; fi",
        );
        assert_eq!(
            env.run(AssetKind::Png, b"x").await,
            Err(WorkerRefusal::Malformed)
        );
        // And after all that, a good job still runs.
        let good = sh(
            "cat >/dev/null; printf '{\"ok\":true,\"media_type\":\"image/png\",\"length\":2}\\nok'",
        );
        assert_eq!(
            good.run(AssetKind::Png, b"input").await.unwrap().output,
            b"ok"
        );
    }
}
