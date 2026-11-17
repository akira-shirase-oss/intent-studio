//! Typed configuration, read once from `INTENT_*` variables and validated with every problem
//! reported together (rust-mastery chapter 12). Secrets live in [`Secret`], whose `Debug` is
//! redacted and which has no `Display`.

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

use intent_db::PoolLimits;
use intent_domain::{
    limits::{
        ADDRESS_LEADS_PER_MINUTE, NOTIFY_ATTEMPT_DEADLINE, NOTIFY_RETRY_BASE, RATE_TABLE_ENTRIES,
        SITE_LEADS_PER_MINUTE,
    },
    ratelimit::Quota,
};

/// A secret string: redacted in `Debug`, no `Display`, read with [`Secret::expose`].
#[derive(Clone)]
pub struct Secret(Box<str>);

impl Secret {
    /// Wraps a value.
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    /// The value. Every call site is a place a secret leaves its wrapper: keep them few.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line (default).
    Json,
    /// Human-readable.
    Pretty,
}

/// Where the server runs. Production is the default: development-only conveniences must be
/// asked for by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The default.
    Production,
    /// A developer's machine or a test: allows the in-memory recorder provider.
    Development,
}

/// Which notification provider adapter to use.
#[derive(Debug, Clone)]
pub enum ProviderConfig {
    /// JSON over HTTP to this endpoint (the provider fixture in development and tests).
    Webhook { url: String },
    /// Accept and count in memory (development only).
    Recorder,
}

/// The notifier's settings.
#[derive(Debug, Clone)]
pub struct NotifyConfig {
    /// The adapter.
    pub provider: ProviderConfig,
    /// One attempt's deadline (D-020: 10 s).
    pub attempt_deadline: Duration,
    /// The retry schedule's base (D-020/D-030: one minute; drills use less).
    pub retry_base: Duration,
    /// A claimed row's lease; longer than the attempt deadline.
    pub lease: Duration,
    /// Attempts in flight at once.
    pub concurrency: usize,
    /// Rows claimed per poll.
    pub batch: u16,
    /// Pause between polls when nothing is due.
    pub poll_interval: Duration,
    /// The notifier's own connection pool: a busy notifier can never take intake's connections.
    pub db_connections: u32,
}

impl NotifyConfig {
    /// The notifier pool's limits: its own size, and a longer wait for a connection than
    /// intake's, because a background record can wait where a visitor should not.
    pub fn pool_limits(&self, intake: intent_db::PoolLimits) -> intent_db::PoolLimits {
        intent_db::PoolLimits {
            max_connections: self.db_connections,
            acquire_timeout: Duration::from_secs(5),
            ..intake
        }
    }
}

/// The studio's untrusted-input settings (I1): the content store and the asset worker.
#[derive(Debug, Clone)]
pub struct StudioConfig {
    /// `INTENT_STORE_DIR`: the content-addressed store's root (D-016). Without it, asset uploads
    /// and reference snapshots answer `503 store_unavailable`; everything else works.
    pub store_dir: Option<PathBuf>,
    /// `INTENT_WORKER_BIN`: the asset worker; by default `intent-worker` beside this binary.
    pub worker_bin: PathBuf,
    /// `INTENT_WORKER_TIMEOUT_MS`: one job's wall time.
    pub worker_timeout: Duration,
    /// `INTENT_WORKER_CONCURRENCY`: jobs at once.
    pub worker_concurrency: usize,
}

/// The whole configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// `INTENT_PROFILE`: `production` (default) or `development`.
    pub profile: Profile,
    /// Listen address.
    pub bind: SocketAddr,
    /// PostgreSQL URL (contains the password).
    pub database_url: Secret,
    /// Pool and statement limits.
    pub db: PoolLimits,
    /// Bound on any request, to response headers.
    pub request_timeout: Duration,
    /// Bound on storing one lead; past it the lead is not acknowledged.
    pub intake_deadline: Duration,
    /// Requests in flight before new ones are shed with 503.
    pub max_in_flight: usize,
    /// Open connections, header-read timeout and write-stall deadline (F1).
    pub connections: crate::conn::ConnectionLimits,
    /// Proxies whose `X-Forwarded-For` is believed.
    pub trusted_proxies: Vec<IpAddr>,
    /// Leads per minute per site (D-019).
    pub site_quota: Quota,
    /// Leads per minute per client address (D-019).
    pub address_quota: Quota,
    /// Entries per rate-limit table (D-019).
    pub rate_table_entries: usize,
    /// The notifier.
    pub notify: NotifyConfig,
    /// A bearer token for `/metrics`; without it only direct requests from the host may read.
    pub metrics_token: Option<Secret>,
    /// The operator's argon2id password hash (PHC string); operator sign-in is off without it.
    pub operator_password_hash: Option<Secret>,
    /// Operator session lifetime.
    pub session_ttl: Duration,
    /// An operator session unused for this long is over.
    pub session_idle: Duration,
    /// Mark the session cookie `Secure`.
    pub cookie_secure: bool,
    /// The built workspace app, served at `/workspace`.
    pub workspace_dir: Option<PathBuf>,
    /// The content store and the asset worker.
    pub studio: StudioConfig,
    /// Log format.
    pub log_format: LogFormat,
    /// `tracing` filter directives.
    pub log_filter: String,
    /// How often readiness checks the database.
    pub db_probe_interval: Duration,
    /// After a shutdown signal: readiness is false this long before the listener stops.
    pub drain_delay: Duration,
    /// After the listener stops: the longest wait for in-flight requests.
    pub shutdown_grace: Duration,
}

/// Every problem found, reported together.
#[derive(Debug, thiserror::Error)]
#[error("invalid configuration ({} problem(s)): {}", .problems.len(), .problems.join("; "))]
pub struct ConfigError {
    /// One line per problem; never a secret's value.
    pub problems: Vec<String>,
}

struct Reader<F> {
    lookup: F,
    problems: Vec<String>,
}

impl<F: Fn(&str) -> Option<String>> Reader<F> {
    fn raw(&self, key: &str) -> Option<String> {
        (self.lookup)(key)
    }

    fn parse<T: FromStr>(&mut self, key: &str, default: T) -> T {
        match self.raw(key) {
            None => default,
            Some(text) => text.trim().parse().unwrap_or_else(|_| {
                self.problems.push(format!("{key}: cannot parse {text:?}"));
                default
            }),
        }
    }

    fn ranged(&mut self, key: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> u64 {
        let value = self.parse(key, default);
        if range.contains(&value) {
            value
        } else {
            self.problems.push(format!(
                "{key}: {value} is outside {}..={}",
                range.start(),
                range.end()
            ));
            default
        }
    }

    fn millis(
        &mut self,
        key: &str,
        default: Duration,
        range: std::ops::RangeInclusive<u64>,
    ) -> Duration {
        let default_ms = u64::try_from(default.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(self.ranged(key, default_ms, range))
    }

    fn secret(&mut self, key: &str) -> Option<Secret> {
        match self.raw(key) {
            None => None,
            Some(value) if value.trim().is_empty() => {
                // Blank almost always means a failed secret mount.
                self.problems.push(format!("{key}: set but blank"));
                None
            }
            Some(value) => Some(Secret::new(value.trim())),
        }
    }

    fn quota(&mut self, key: &str, default: u32) -> Quota {
        let value = self.ranged(key, u64::from(default), 1..=1_000_000);
        u32::try_from(value)
            .ok()
            .and_then(Quota::per_minute)
            .or_else(|| Quota::per_minute(default))
            .unwrap_or_else(unreachable_quota)
    }
}

/// The weakest argon2id parameters accepted (OWASP's baseline): 19 MiB of memory, 2 iterations.
const ARGON2_MIN_MEMORY_KIB: u32 = 19_456;
/// See [`ARGON2_MIN_MEMORY_KIB`].
const ARGON2_MIN_ITERATIONS: u32 = 2;

/// An argon2id PHC string at or above the parameter floor. Problems never quote the hash.
fn check_password_hash(phc: &str) -> Result<(), &'static str> {
    let parsed = argon2::PasswordHash::new(phc)
        .map_err(|_| "not a valid PHC string (make one with `intent-server hash-password`)")?;
    if parsed.algorithm.as_str() != "argon2id" {
        return Err("expected an argon2id hash (`intent-server hash-password`)");
    }
    let params = argon2::Params::try_from(&parsed)
        .map_err(|_| "not a valid PHC string (its parameters do not parse)")?;
    if params.m_cost() < ARGON2_MIN_MEMORY_KIB || params.t_cost() < ARGON2_MIN_ITERATIONS {
        return Err(
            "argon2id parameters below the floor (19 MiB of memory, 2 iterations; `intent-server hash-password` meets it)",
        );
    }
    Ok(())
}

/// The defaults are non-zero constants, so a quota always builds.
fn unreachable_quota() -> Quota {
    Quota::per_minute(1).unwrap_or_else(|| unreachable!("invariant: 1 is a valid quota"))
}

impl Config {
    /// Reads the configuration through `lookup` (the process environment in production, a map
    /// in tests).
    #[expect(
        clippy::too_many_lines,
        reason = "every variable in one flat, reviewable list"
    )]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut env = Reader {
            lookup,
            problems: Vec::new(),
        };
        let profile = match env.raw("INTENT_PROFILE").as_deref().map(str::trim) {
            None | Some("production") => Profile::Production,
            Some("development") => Profile::Development,
            Some(other) => {
                env.problems.push(format!(
                    "INTENT_PROFILE: expected `production` or `development`, got {other:?}"
                ));
                Profile::Production
            }
        };
        let bind = env.parse("INTENT_BIND", SocketAddr::from(([127, 0, 0, 1], 59400)));
        let database_url = env.secret("INTENT_DATABASE_URL");
        if database_url.is_none() && env.raw("INTENT_DATABASE_URL").is_none() {
            env.problems.push("INTENT_DATABASE_URL: required".into());
        }
        let db = PoolLimits {
            max_connections: u32::try_from(env.ranged("INTENT_DB_MAX_CONNECTIONS", 16, 1..=500))
                .unwrap_or(16),
            acquire_timeout: env.millis(
                "INTENT_DB_ACQUIRE_TIMEOUT_MS",
                Duration::from_secs(1),
                10..=30_000,
            ),
            statement_timeout: env.millis(
                "INTENT_DB_STATEMENT_TIMEOUT_MS",
                Duration::from_secs(3),
                10..=60_000,
            ),
        };
        let request_timeout = env.millis(
            "INTENT_REQUEST_TIMEOUT_MS",
            Duration::from_secs(5),
            100..=120_000,
        );
        let intake_deadline = env.millis(
            "INTENT_INTAKE_DEADLINE_MS",
            Duration::from_secs(3),
            50..=60_000,
        );
        if intake_deadline >= request_timeout {
            env.problems.push(
                "INTENT_INTAKE_DEADLINE_MS must be smaller than INTENT_REQUEST_TIMEOUT_MS".into(),
            );
        }
        let max_connections =
            usize::try_from(env.ranged("INTENT_MAX_CONNECTIONS", 1024, 1..=100_000))
                .unwrap_or(1024);
        let connections = crate::conn::ConnectionLimits {
            max_connections,
            header_read_timeout: env.millis(
                "INTENT_HEADER_READ_TIMEOUT_MS",
                Duration::from_secs(10),
                100..=120_000,
            ),
            write_stall: env.millis(
                "INTENT_WRITE_STALL_TIMEOUT_MS",
                Duration::from_secs(10),
                100..=120_000,
            ),
            max_requests_per_connection: u32::try_from(env.ranged(
                "INTENT_MAX_REQUESTS_PER_CONNECTION",
                1_000,
                1..=1_000_000,
            ))
            .unwrap_or(1_000),
            max_age: env.millis(
                "INTENT_MAX_CONNECTION_AGE_MS",
                Duration::from_mins(5),
                1_000..=86_400_000,
            ),
            max_per_address: usize::try_from(env.ranged(
                "INTENT_MAX_CONNECTIONS_PER_ADDRESS",
                u64::try_from((max_connections / 4).clamp(1, 64)).unwrap_or(64),
                1..=100_000,
            ))
            .unwrap_or(64),
        };
        if connections.max_per_address > connections.max_connections {
            env.problems.push(
                "INTENT_MAX_CONNECTIONS_PER_ADDRESS must not exceed INTENT_MAX_CONNECTIONS".into(),
            );
        }
        let max_in_flight =
            usize::try_from(env.ranged("INTENT_MAX_IN_FLIGHT", 1024, 1..=100_000)).unwrap_or(1024);
        let mut trusted_proxies = Vec::new();
        for entry in env
            .raw("INTENT_TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
        {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Ok(ip) = entry.parse::<IpAddr>() {
                trusted_proxies.push(ip);
            } else {
                env.problems.push(format!(
                    "INTENT_TRUSTED_PROXIES: {entry:?} is not an IP address"
                ));
            }
        }
        let site_quota = env.quota("INTENT_LIMIT_SITE_PER_MINUTE", SITE_LEADS_PER_MINUTE);
        let address_quota = env.quota("INTENT_LIMIT_ADDRESS_PER_MINUTE", ADDRESS_LEADS_PER_MINUTE);
        let rate_table_entries = usize::try_from(env.ranged(
            "INTENT_LIMIT_TABLE_ENTRIES",
            u64::try_from(RATE_TABLE_ENTRIES).unwrap_or(100_000),
            1_000..=10_000_000,
        ))
        .unwrap_or(RATE_TABLE_ENTRIES);

        let provider = match env.raw("INTENT_NOTIFY_PROVIDER").as_deref().map(str::trim) {
            Some("webhook") => {
                if let Some(url) = env.raw("INTENT_NOTIFY_WEBHOOK_URL") {
                    ProviderConfig::Webhook { url }
                } else {
                    env.problems.push(
                        "INTENT_NOTIFY_WEBHOOK_URL: required when INTENT_NOTIFY_PROVIDER=webhook"
                            .into(),
                    );
                    ProviderConfig::Recorder
                }
            }
            Some("recorder") => {
                // The recorder accepts and forgets every notification: in production that would
                // be a provider that silently delivers nothing.
                if profile != Profile::Development {
                    env.problems.push(
                        "INTENT_NOTIFY_PROVIDER=recorder discards notifications and is for development only (set INTENT_PROFILE=development)".into(),
                    );
                }
                ProviderConfig::Recorder
            }
            Some(other) => {
                env.problems.push(format!(
                    "INTENT_NOTIFY_PROVIDER: expected `webhook` or `recorder`, got {other:?}"
                ));
                ProviderConfig::Recorder
            }
            None => {
                // No silent default: a server that quietly drops notifications is worse than one
                // that refuses to start.
                env.problems
                    .push("INTENT_NOTIFY_PROVIDER: required (`webhook` or `recorder`)".into());
                ProviderConfig::Recorder
            }
        };
        let attempt_deadline = env.millis(
            "INTENT_NOTIFY_ATTEMPT_TIMEOUT_MS",
            NOTIFY_ATTEMPT_DEADLINE,
            50..=60_000,
        );
        let retry_base = env.millis(
            "INTENT_NOTIFY_RETRY_BASE_MS",
            NOTIFY_RETRY_BASE,
            1..=600_000,
        );
        let lease = env.millis(
            "INTENT_NOTIFY_LEASE_MS",
            Duration::from_secs(30),
            100..=600_000,
        );
        if lease <= attempt_deadline {
            env.problems.push(
                "INTENT_NOTIFY_LEASE_MS must be longer than INTENT_NOTIFY_ATTEMPT_TIMEOUT_MS"
                    .into(),
            );
        }
        let notify = NotifyConfig {
            provider,
            attempt_deadline,
            retry_base,
            lease,
            concurrency: usize::try_from(env.ranged("INTENT_NOTIFY_CONCURRENCY", 32, 1..=1024))
                .unwrap_or(32),
            batch: u16::try_from(env.ranged("INTENT_NOTIFY_BATCH", 64, 1..=1000)).unwrap_or(64),
            poll_interval: env.millis(
                "INTENT_NOTIFY_POLL_MS",
                Duration::from_millis(250),
                10..=60_000,
            ),
            db_connections: u32::try_from(env.ranged("INTENT_NOTIFY_DB_CONNECTIONS", 8, 1..=100))
                .unwrap_or(8),
        };

        // Each attempt needs a connection to claim and to record; more than eight attempts per
        // connection and the records queue for the pool (D-034).
        if notify.concurrency > 8 * notify.db_connections as usize {
            env.problems.push(format!(
                "INTENT_NOTIFY_CONCURRENCY ({}) is more than 8 attempts per notifier connection (INTENT_NOTIFY_DB_CONNECTIONS = {})",
                notify.concurrency, notify.db_connections
            ));
        }
        let metrics_token = env.secret("INTENT_METRICS_TOKEN");
        if metrics_token
            .as_ref()
            .is_some_and(|t| t.expose().len() < 16)
        {
            env.problems
                .push("INTENT_METRICS_TOKEN: at least 16 characters".into());
        }
        let operator_password_hash = env.secret("INTENT_OPERATOR_PASSWORD_HASH");
        if let Some(hash) = &operator_password_hash
            && let Err(problem) = check_password_hash(hash.expose())
        {
            env.problems
                .push(format!("INTENT_OPERATOR_PASSWORD_HASH: {problem}"));
        }
        let session_ttl = Duration::from_secs(env.ranged(
            "INTENT_SESSION_TTL_SECS",
            12 * 3600,
            60..=7 * 24 * 3600,
        ));
        let session_idle =
            Duration::from_secs(env.ranged("INTENT_SESSION_IDLE_SECS", 30 * 60, 1..=24 * 3600));
        let cookie_secure = env.parse("INTENT_COOKIE_SECURE", true);
        let workspace_dir = env.raw("INTENT_WORKSPACE_DIR").map(PathBuf::from);
        let studio = StudioConfig {
            store_dir: env
                .raw("INTENT_STORE_DIR")
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from),
            worker_bin: env.raw("INTENT_WORKER_BIN").map_or_else(
                || {
                    std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(|dir| dir.join("intent-worker")))
                        .unwrap_or_else(|| PathBuf::from("intent-worker"))
                },
                PathBuf::from,
            ),
            worker_timeout: env.millis(
                "INTENT_WORKER_TIMEOUT_MS",
                intent_render::DEFAULT_DEADLINE,
                500..=60_000,
            ),
            worker_concurrency: usize::try_from(env.ranged(
                "INTENT_WORKER_CONCURRENCY",
                intent_render::DEFAULT_CONCURRENCY as u64,
                1..=16,
            ))
            .unwrap_or(intent_render::DEFAULT_CONCURRENCY),
        };
        let log_format = match env.raw("INTENT_LOG_FORMAT").as_deref() {
            None | Some("json") => LogFormat::Json,
            Some("pretty") => LogFormat::Pretty,
            Some(other) => {
                env.problems.push(format!(
                    "INTENT_LOG_FORMAT: expected `json` or `pretty`, got {other:?}"
                ));
                LogFormat::Json
            }
        };
        let log_filter = env
            .raw("INTENT_LOG")
            .unwrap_or_else(|| "info,sqlx=warn".to_owned());
        let db_probe_interval = env.millis(
            "INTENT_DB_PROBE_INTERVAL_MS",
            Duration::from_millis(500),
            50..=60_000,
        );
        let drain_delay = env.millis("INTENT_DRAIN_DELAY_MS", Duration::from_secs(2), 0..=60_000);
        let shutdown_grace = env.millis(
            "INTENT_SHUTDOWN_GRACE_MS",
            Duration::from_secs(10),
            100..=600_000,
        );

        match database_url {
            Some(database_url) if env.problems.is_empty() => Ok(Self {
                profile,
                bind,
                database_url,
                db,
                request_timeout,
                intake_deadline,
                max_in_flight,
                connections,
                trusted_proxies,
                site_quota,
                address_quota,
                rate_table_entries,
                notify,
                metrics_token,
                operator_password_hash,
                session_ttl,
                session_idle,
                cookie_secure,
                workspace_dir,
                studio,
                log_format,
                log_filter,
                db_probe_interval,
                drain_delay,
                shutdown_grace,
            }),
            _ => Err(ConfigError {
                problems: env.problems,
            }),
        }
    }

    /// Reads the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var_os(key).map(|v| v.to_string_lossy().into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_lookup(move |key| map.get(key).cloned())
    }

    const MINIMAL: [(&str, &str); 3] = [
        (
            "INTENT_DATABASE_URL",
            "postgres://u:hunter2@127.0.0.1:59432/intent",
        ),
        ("INTENT_NOTIFY_PROVIDER", "recorder"),
        ("INTENT_PROFILE", "development"),
    ];

    fn phc(memory_kib: u32, iterations: u32) -> String {
        use argon2::{
            Algorithm, Argon2, Params, PasswordHasher as _, Version, password_hash::SaltString,
        };
        let params = Params::new(memory_kib, iterations, 1, None).unwrap();
        let salt = SaltString::encode_b64(b"0123456789abcdef").unwrap();
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(b"password", &salt)
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_weak_or_malformed_password_hash_is_refused() {
        let with = |hash: &str| {
            let mut pairs: Vec<(&str, String)> =
                MINIMAL.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
            pairs.push(("INTENT_OPERATOR_PASSWORD_HASH", hash.to_owned()));
            let map: HashMap<String, String> =
                pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            Config::from_lookup(move |key| map.get(key).cloned())
        };
        assert!(
            with(&phc(19_456, 2)).is_ok(),
            "the OWASP baseline is accepted"
        );
        let weak = with(&phc(8, 1)).unwrap_err().to_string();
        assert!(weak.contains("below the floor"), "{weak}");
        let short = with(&phc(19_456, 1)).unwrap_err().to_string();
        assert!(short.contains("below the floor"), "{short}");
        let garbage = with("$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA")
            .unwrap_err()
            .to_string();
        assert!(garbage.contains("not a valid"), "{garbage}");
    }

    #[test]
    fn one_address_gets_at_most_a_quarter_of_a_small_cap() {
        let per_address = |cap: &str| {
            let mut pairs = MINIMAL.to_vec();
            pairs.push(("INTENT_MAX_CONNECTIONS", cap));
            config(&pairs).unwrap().connections.max_per_address
        };
        // At a cap of 20, one address may not take all 20 and lock newcomers out.
        assert_eq!(per_address("20"), 5);
        assert_eq!(per_address("1024"), 64);
        assert_eq!(per_address("100"), 25);
        assert_eq!(per_address("2"), 1);
        assert_eq!(per_address("1"), 1);
    }

    #[test]
    fn the_notifier_pool_is_sized_for_its_concurrency() {
        let defaults = config(&MINIMAL).unwrap();
        assert!(
            defaults.notify.concurrency <= 8 * defaults.notify.db_connections as usize,
            "default concurrency {} on {} connections",
            defaults.notify.concurrency,
            defaults.notify.db_connections
        );
        let mut starved = MINIMAL.to_vec();
        starved.push(("INTENT_NOTIFY_CONCURRENCY", "64"));
        starved.push(("INTENT_NOTIFY_DB_CONNECTIONS", "2"));
        let text = config(&starved).unwrap_err().to_string();
        assert!(text.contains("INTENT_NOTIFY_CONCURRENCY"), "{text}");
    }

    #[test]
    fn the_recorder_is_refused_outside_development() {
        let url = (
            "INTENT_DATABASE_URL",
            "postgres://u:p@127.0.0.1:59432/intent",
        );
        let text = config(&[url, ("INTENT_NOTIFY_PROVIDER", "recorder")])
            .unwrap_err()
            .to_string();
        assert!(
            text.contains("recorder") && text.contains("INTENT_PROFILE=development"),
            "{text}"
        );
        assert!(
            config(&[
                url,
                ("INTENT_NOTIFY_PROVIDER", "recorder"),
                ("INTENT_PROFILE", "development")
            ])
            .is_ok()
        );
        let unknown = config(&[
            url,
            ("INTENT_NOTIFY_PROVIDER", "recorder"),
            ("INTENT_PROFILE", "staging"),
        ]);
        assert!(unknown.unwrap_err().to_string().contains("INTENT_PROFILE"));
    }

    #[test]
    fn defaults_follow_the_decisions() {
        let c = config(&MINIMAL).unwrap();
        assert_eq!(c.bind, "127.0.0.1:59400".parse().unwrap());
        assert_eq!(c.site_quota.get(), 60);
        assert_eq!(c.address_quota.get(), 5);
        assert_eq!(c.rate_table_entries, 100_000);
        assert_eq!(c.notify.attempt_deadline, Duration::from_secs(10));
        assert_eq!(c.notify.retry_base, Duration::from_mins(1));
        assert!(c.operator_password_hash.is_none());
    }

    #[test]
    fn every_problem_is_reported_together() {
        let error = config(&[
            ("INTENT_LIMIT_SITE_PER_MINUTE", "0"),
            ("INTENT_LOG_FORMAT", "xml"),
            ("INTENT_NOTIFY_PROVIDER", "smtp"),
            ("INTENT_TRUSTED_PROXIES", "10.0.0.1, nope"),
        ])
        .unwrap_err();
        let text = error.to_string();
        for expected in [
            "INTENT_DATABASE_URL: required",
            "INTENT_LIMIT_SITE_PER_MINUTE: 0 is outside",
            "INTENT_LOG_FORMAT",
            "INTENT_NOTIFY_PROVIDER",
            "\"nope\" is not an IP address",
        ] {
            assert!(text.contains(expected), "{expected} missing from {text}");
        }
    }

    #[test]
    fn a_webhook_needs_its_url_and_the_lease_must_outlast_an_attempt() {
        let mut pairs = MINIMAL.to_vec();
        pairs[1] = ("INTENT_NOTIFY_PROVIDER", "webhook");
        pairs.push(("INTENT_NOTIFY_LEASE_MS", "5000"));
        let text = config(&pairs).unwrap_err().to_string();
        assert!(text.contains("INTENT_NOTIFY_WEBHOOK_URL"));
        assert!(text.contains("INTENT_NOTIFY_LEASE_MS must be longer"));
    }

    #[test]
    fn secrets_never_print() {
        let mut pairs = MINIMAL.to_vec();
        let real = phc(19_456, 2);
        pairs.push(("INTENT_OPERATOR_PASSWORD_HASH", real.as_str()));
        let c = config(&pairs).unwrap();
        let printed = format!("{c:?}");
        assert!(!printed.contains("hunter2"));
        assert!(!printed.contains("argon2id"));
        let blank = config(&[
            ("INTENT_DATABASE_URL", " "),
            ("INTENT_NOTIFY_PROVIDER", "recorder"),
        ]);
        assert!(blank.unwrap_err().to_string().contains("set but blank"));
    }

    #[test]
    fn a_hash_that_is_not_argon2id_is_refused() {
        let mut pairs = MINIMAL.to_vec();
        pairs.push(("INTENT_OPERATOR_PASSWORD_HASH", "plaintext-password"));
        let text = config(&pairs).unwrap_err().to_string();
        assert!(text.contains("not a valid PHC"));
        assert!(!text.contains("plaintext-password"));
    }
}
