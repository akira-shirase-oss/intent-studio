//! The drill harness: a fresh database on the local stack, the real `intent-server` binary as a
//! child process with its logs captured, the provider fixture in-process on a real socket, a
//! load generator, reconciliation and the log scanner (D-029).
//!
//! Every listener stays inside 127.0.0.1:59410–59479. The stack is `scripts/stack.sh up`
//! (Postgres on 127.0.0.1:59432); tests that need it are `#[ignore]`d and run by
//! `scripts/drills.sh`.

#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test binary uses a different part of the harness"
)]
#![allow(
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test support: panics are test failures"
)]

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};

use intent_provider_fixture::{Fixture, Mode};
use sqlx::{Connection as _, Row as _};
use uuid::Uuid;

pub const ADMIN_URL: &str = "postgres://intent:intent-dev@127.0.0.1:59432/intent";
pub const DB_PASSWORD: &str = "intent-dev";

static NEXT_PORT: AtomicU16 = AtomicU16::new(59410);

/// A free port in 59410..=59479.
pub fn free_port() -> u16 {
    for _ in 0..200 {
        let mut port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);
        if port > 59479 {
            NEXT_PORT.store(59411, Ordering::SeqCst);
            port = 59410;
        }
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in 59410..=59479");
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn server_binary() -> &'static str {
    env!("CARGO_BIN_EXE_intent-server")
}

/// A database of its own, migrated and seeded with `sites` demo sites.
pub struct TestDb {
    pub name: String,
    pub url: String,
}

impl TestDb {
    pub async fn create(sites: u32) -> Self {
        let name = format!("intent_t_{}", Uuid::now_v7().simple());
        let mut admin = sqlx::PgConnection::connect(ADMIN_URL)
            .await
            .expect("the stack is up: scripts/stack.sh up");
        // The name is generated here from a UUID: no outside input reaches this statement.
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut admin)
            .await
            .unwrap();
        let url = format!("postgres://intent:{DB_PASSWORD}@127.0.0.1:59432/{name}");
        for args in [
            vec!["migrate".to_owned()],
            vec!["seed".into(), "--sites".into(), sites.to_string()],
        ] {
            let status = Command::new(server_binary())
                .args(&args)
                .env("INTENT_DATABASE_URL", &url)
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "intent-server {args:?} failed");
        }
        Self { name, url }
    }

    pub async fn connect(&self) -> sqlx::PgConnection {
        sqlx::PgConnection::connect(&self.url).await.unwrap()
    }

    pub async fn drop_database(self) {
        if let Ok(mut admin) = sqlx::PgConnection::connect(ADMIN_URL).await {
            let sql = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name);
            let _ = sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut admin)
                .await;
        }
    }

    /// Stored key → lead ids, for keys starting with `c:<prefix>`.
    pub async fn leads_with_prefix(&self, prefix: &str) -> HashMap<String, Vec<Uuid>> {
        let mut conn = self.connect().await;
        let rows = sqlx::query("SELECT submission_key, id FROM leads WHERE submission_key LIKE $1")
            .bind(format!("c:{prefix}%"))
            .fetch_all(&mut conn)
            .await
            .unwrap();
        let mut map: HashMap<String, Vec<Uuid>> = HashMap::new();
        for row in rows {
            map.entry(row.get::<String, _>(0))
                .or_default()
                .push(row.get(1));
        }
        map
    }

    /// `(state, attempts)` → rows, over the whole outbox.
    pub async fn outbox_tally(&self) -> BTreeMap<(String, i16), i64> {
        let mut conn = self.connect().await;
        sqlx::query("SELECT state, attempts, count(*) FROM notification_outbox GROUP BY 1, 2")
            .fetch_all(&mut conn)
            .await
            .unwrap()
            .into_iter()
            .map(|row| ((row.get(0), row.get(1)), row.get(2)))
            .collect()
    }

    pub async fn scalar(&self, sql: &'static str) -> i64 {
        let mut conn = self.connect().await;
        sqlx::query_scalar(sql).fetch_one(&mut conn).await.unwrap()
    }

    /// Waits until no outbox row is pending, or panics after `limit`.
    pub async fn wait_outbox_terminal(&self, limit: Duration) {
        let start = Instant::now();
        loop {
            let pending = self
                .scalar("SELECT count(*) FROM notification_outbox WHERE state = 'pending'")
                .await;
            if pending == 0 {
                return;
            }
            assert!(
                start.elapsed() < limit,
                "{pending} outbox row(s) still pending after {limit:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// The provider fixture on a real socket, in this process.
pub struct ProviderFixture {
    pub fixture: Fixture,
    pub url: String,
    stop: tokio_util_stop::Stop,
}

/// A tiny stop signal without pulling tokio-util into the tests.
mod tokio_util_stop {
    pub struct Stop(pub Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Stop {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
}

impl ProviderFixture {
    pub async fn start(mode: Mode) -> Self {
        // On macOS an outgoing connection may take a probed port (its local port comes from
        // the same range) before this bind: try another.
        let mut bound = None;
        for _ in 0..20 {
            let port = free_port();
            if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                bound = Some((port, listener));
                break;
            }
        }
        let (port, listener) = bound.expect("no port in 59410..=59479 could be bound");
        let fixture = Fixture::new(mode, Duration::from_secs(30));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let served = fixture.clone();
        tokio::spawn(async move {
            let _ = intent_provider_fixture::serve(listener, served, async {
                let _ = rx.await;
            })
            .await;
        });
        Self {
            fixture,
            url: format!("http://127.0.0.1:{port}/v1/notify"),
            stop: tokio_util_stop::Stop(Some(tx)),
        }
    }
}

/// The real server binary, with its stdout and stderr captured to a file.
pub struct Server {
    child: Option<Child>,
    pub address: SocketAddr,
    pub base: String,
    pub log_path: PathBuf,
    env: Vec<(String, String)>,
    restarts: u32,
}

/// Settings for a drill server: fast retries so a drill sees the whole D-030 shape in seconds.
pub fn drill_env(db: &TestDb, provider_url: &str) -> Vec<(String, String)> {
    [
        ("INTENT_DATABASE_URL", db.url.as_str()),
        ("INTENT_NOTIFY_PROVIDER", "webhook"),
        ("INTENT_NOTIFY_WEBHOOK_URL", provider_url),
        ("INTENT_NOTIFY_RETRY_BASE_MS", "20"),
        ("INTENT_NOTIFY_ATTEMPT_TIMEOUT_MS", "300"),
        ("INTENT_NOTIFY_LEASE_MS", "2000"),
        ("INTENT_NOTIFY_POLL_MS", "50"),
        ("INTENT_DB_PROBE_INTERVAL_MS", "200"),
        ("INTENT_DRAIN_DELAY_MS", "0"),
        (
            "INTENT_LOG",
            "debug,sqlx=warn,hyper=info,h2=info,reqwest=info",
        ),
        ("INTENT_COOKIE_SECURE", "false"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

impl Server {
    pub async fn start(env: Vec<(String, String)>) -> Self {
        let dir = std::env::temp_dir().join(format!("intent-drill-{}", Uuid::now_v7().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        // A probed port can be taken by an outgoing connection before the server binds it
        // (macOS gives local ports from the same range): if the server cannot bind, it exits
        // at once, and the next port is tried.
        for _ in 0..5 {
            let port = free_port();
            let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let mut env = env.clone();
            env.push(("INTENT_BIND".into(), address.to_string()));
            let mut server = Self {
                child: None,
                address,
                base: format!("http://{address}"),
                log_path: dir.join("server.log"),
                env,
                restarts: 0,
            };
            server.spawn();
            if server.became_ready(Duration::from_secs(20)).await {
                return server;
            }
            server.kill();
        }
        panic!(
            "the server never started:\n{}",
            std::fs::read_to_string(dir.join("server.log")).unwrap_or_default()
        );
    }

    /// Waits for readiness; false if the process exited first (e.g. its port was taken).
    async fn became_ready(&mut self, limit: Duration) -> bool {
        let client = reqwest::Client::new();
        let start = Instant::now();
        loop {
            if let Some(child) = self.child.as_mut()
                && child.try_wait().ok().flatten().is_some()
            {
                self.child = None;
                return false;
            }
            if let Ok(r) = client
                .get(format!("{}/readyz", self.base))
                .timeout(Duration::from_secs(1))
                .send()
                .await
                && r.status() == 200
            {
                return true;
            }
            assert!(
                start.elapsed() < limit,
                "server not ready after {limit:?}:\n{}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn spawn(&mut self) {
        let log = File::options()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .unwrap();
        let child = Command::new(server_binary())
            .arg("serve")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        self.child = Some(child);
    }

    pub async fn wait_ready(&self, limit: Duration) {
        let client = reqwest::Client::new();
        let start = Instant::now();
        loop {
            if let Ok(r) = client
                .get(format!("{}/readyz", self.base))
                .timeout(Duration::from_secs(1))
                .send()
                .await
                && r.status() == 200
            {
                return;
            }
            assert!(
                start.elapsed() < limit,
                "server not ready after {limit:?}:\n{}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// SIGKILL: no drain, no cleanup.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// SIGTERM, then wait for exit.
    pub fn terminate(&mut self) -> std::process::ExitStatus {
        let mut child = self.child.take().expect("running");
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        child.wait().unwrap()
    }

    /// Starts the binary again on the same address and database.
    pub async fn restart(&mut self) {
        assert!(self.child.is_none(), "stop it first");
        self.restarts += 1;
        self.spawn();
        self.wait_ready(Duration::from_secs(20)).await;
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill();
    }
}

/// One deterministic synthetic lead. All of its values are needles for the log scanner.
#[derive(Debug, Clone)]
pub struct LeadSpec {
    pub submission_id: String,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub message: String,
}

impl LeadSpec {
    pub fn new(run: &str, n: u64) -> Self {
        Self {
            // At least 8 characters (the domain's minimum).
            submission_id: format!("{run}-n{n:05}"),
            name: format!("Drillname {run} {n}"),
            email: format!("lead.{run}.{n}@example.test"),
            phone: format!("+1 555 01{:02} {:04}", n % 100, n % 10_000),
            message: format!("Please quote job {run}/{n}. Second line follows.\nThanks"),
        }
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "submission_id": self.submission_id,
            "name": self.name,
            "email": self.email,
            "phone": self.phone,
            "message": self.message,
            "consent": true,
            "consent_notice": "privacy-v1",
        })
    }

    /// Strings that must never appear in a log.
    pub fn needles(&self) -> [&str; 4] {
        [&self.name, &self.email, &self.phone, &self.message[..20]]
    }
}

/// What one submission got.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub n: u64,
    pub status: Option<u16>,
    pub lead_id: Option<Uuid>,
    pub duplicate: bool,
    pub retry_after: Option<String>,
    pub code: Option<String>,
    pub latency: Duration,
    pub at: Duration,
}

impl Outcome {
    pub fn acknowledged(&self) -> bool {
        matches!(self.status, Some(200 | 201)) && self.lead_id.is_some()
    }
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(256)
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap()
}

/// Submits one lead to `site` as client address `forwarded` (when the server trusts the peer).
pub async fn submit(
    client: &reqwest::Client,
    base: &str,
    site: Uuid,
    body: &serde_json::Value,
    forwarded: Option<&str>,
    n: u64,
    clock: Instant,
) -> Outcome {
    let started = Instant::now();
    let mut request = client
        .post(format!("{base}/v1/sites/{site}/leads"))
        .json(body);
    if let Some(address) = forwarded {
        request = request.header("x-forwarded-for", address);
    }
    let at = clock.elapsed();
    match request.send().await {
        Err(_) => Outcome {
            n,
            status: None,
            lead_id: None,
            duplicate: false,
            retry_after: None,
            code: None,
            latency: started.elapsed(),
            at,
        },
        Ok(response) => {
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            Outcome {
                n,
                status: Some(status),
                lead_id: body["lead_id"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok()),
                duplicate: body["duplicate"].as_bool().unwrap_or(false),
                retry_after,
                code: body["error"]["code"].as_str().map(str::to_owned),
                latency: started.elapsed(),
                at,
            }
        }
    }
}

/// The site a drill lead goes to (spread over `sites`), and the client address it comes from.
pub fn site_for(n: u64, sites: u32) -> Uuid {
    intent_db::demo_site(u32::try_from(n % u64::from(sites)).unwrap()).as_uuid()
}

pub fn address_for(n: u64) -> String {
    format!("10.{}.{}.{}", (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff)
}

/// Submits leads `range` at `rate` per second (open model: each starts on schedule whether or
/// not earlier ones finished), every `resubmit_every`-th one a resubmission of an earlier lead.
pub struct Load {
    pub run: String,
    pub sites: u32,
    pub rate: u32,
    pub resubmit_every: Option<u64>,
}

impl Load {
    pub async fn drive(
        &self,
        base: &str,
        range: std::ops::Range<u64>,
        clock: Instant,
    ) -> Vec<Outcome> {
        let client = client();
        let results = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = tokio::task::JoinSet::new();
        let mut interval = tokio::time::interval(Duration::from_secs(1) / self.rate);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        for n in range {
            interval.tick().await;
            let target = match self.resubmit_every {
                Some(every) if n % every == every - 1 && n >= 10 => n - 10,
                _ => n,
            };
            let spec = LeadSpec::new(&self.run, target);
            let site = site_for(target, self.sites);
            let client = client.clone();
            let base = base.to_owned();
            let results = results.clone();
            tasks.spawn(async move {
                let outcome = submit(
                    &client,
                    &base,
                    site,
                    &spec.json(),
                    Some(&address_for(n)),
                    target,
                    clock,
                )
                .await;
                results.lock().unwrap().push(outcome);
            });
        }
        while tasks.join_next().await.is_some() {}
        Arc::try_unwrap(results).unwrap().into_inner().unwrap()
    }
}

/// Retries every unacknowledged lead (same id, same content) until it is acknowledged.
pub async fn resend_until_acknowledged(
    base: &str,
    run: &str,
    sites: u32,
    ns: &[u64],
) -> Vec<Outcome> {
    let client = client();
    let clock = Instant::now();
    let mut done = Vec::new();
    for &n in ns {
        let spec = LeadSpec::new(run, n);
        let mut last = None;
        for attempt in 0..40_u64 {
            let outcome = submit(
                &client,
                base,
                site_for(n, sites),
                &spec.json(),
                Some(&address_for(n + 1_000_000 + attempt)),
                n,
                clock,
            )
            .await;
            if outcome.acknowledged() {
                last = Some(outcome);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        done.push(
            last.unwrap_or_else(|| panic!("lead {run}-{n} never acknowledged after recovery")),
        );
    }
    done
}

/// Every acknowledged lead is stored exactly once under the lead id it was acknowledged with,
/// and every resubmission was answered with the same lead. Returns the acknowledged set.
pub async fn assert_acknowledged_leads_stored_once(
    db: &TestDb,
    run: &str,
    outcomes: &[Outcome],
) -> HashMap<u64, Uuid> {
    let mut acknowledged: HashMap<u64, Uuid> = HashMap::new();
    for outcome in outcomes.iter().filter(|o| o.acknowledged()) {
        let lead = outcome.lead_id.unwrap();
        if let Some(previous) = acknowledged.insert(outcome.n, lead) {
            assert_eq!(
                previous, lead,
                "lead {run}-{} acknowledged as two different leads",
                outcome.n
            );
        }
    }
    let stored = db.leads_with_prefix(&format!("{run}-")).await;
    for (n, lead) in &acknowledged {
        let ids = stored
            .get(&format!("c:{}", LeadSpec::new(run, *n).submission_id))
            .unwrap_or_else(|| panic!("acknowledged lead {run}-{n} is not stored"));
        assert_eq!(ids, &vec![*lead], "lead {run}-{n} stored as {ids:?}");
    }
    for (key, ids) in &stored {
        assert_eq!(ids.len(), 1, "{key} is stored {} times", ids.len());
    }
    acknowledged
}

pub fn percentile(latencies: &mut [Duration], p: f64) -> Duration {
    latencies.sort();
    if latencies.is_empty() {
        return Duration::ZERO;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "an index into a short test vector"
    )]
    let index = ((latencies.len() as f64 - 1.0) * p).round() as usize;
    latencies[index]
}

/// Lines of `log` holding a secret shape or any `needle` (D-029).
pub fn scan_for_leaks(log: &str, needles: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for (number, line) in log.lines().enumerate() {
        let lower = line.to_ascii_lowercase();
        let shape = lower.contains("bearer ")
            || line.contains("sk-")
            || line.contains("sk_live_")
            || line.contains("$argon2")
            || line.contains("postgres://")
            || line.contains(DB_PASSWORD)
            || has_long_hex(line, 64);
        if shape || needles.iter().any(|n| !n.is_empty() && line.contains(n)) {
            found.push(format!(
                "line {}: {}",
                number + 1,
                line.chars().take(300).collect::<String>()
            ));
        }
    }
    found
}

fn has_long_hex(line: &str, length: usize) -> bool {
    let mut run = 0;
    for b in line.bytes() {
        if b.is_ascii_hexdigit() {
            run += 1;
            if run >= length {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// The log-capture check, with its own proof: the scanner must flag a leak planted into this
/// very log before its clean verdict on the real log counts.
pub fn assert_no_leaks(log: &str, needles: &[&str]) {
    assert!(
        log.contains("\"listening\""),
        "the log capture is empty or broken:\n{log}"
    );
    // Prove the scanner on every needle, one at a time, in this very log: a needle it could not
    // see (empty, or spanning lines) would otherwise make a clean verdict meaningless.
    assert!(
        !needles.is_empty(),
        "no needles: the check would prove nothing"
    );
    for needle in needles {
        let planted = format!("{log}\n{{\"message\":\"planted\",\"value\":\"{needle}\"}}\n");
        let before = scan_for_leaks(log, &[needle]).len();
        let after = scan_for_leaks(&planted, &[needle]).len();
        assert!(
            !needle.is_empty() && after > before,
            "the scanner cannot catch the needle {needle:?} in a log line"
        );
    }
    let leaks = scan_for_leaks(log, needles);
    assert!(
        leaks.is_empty(),
        "secrets or lead PII in the server log:\n{}",
        leaks.join("\n")
    );
}

/// `docker compose` for this repository's stack.
pub fn compose(args: &[&str]) {
    let file = repo_root().join("deploy/compose.yaml");
    let status = Command::new("docker")
        .args(["compose", "-p", "intent-studio", "-f"])
        .arg(&file)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "docker compose {args:?} failed");
}

pub fn fixture_mode(fixture: &ProviderFixture, mode: Mode) {
    fixture.fixture.set_mode(mode);
}
