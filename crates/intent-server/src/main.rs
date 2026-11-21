//! `intent-server`: serve, check the configuration, migrate, seed demo data, hash the operator
//! password, reconcile a run.

use std::{io::BufRead as _, path::PathBuf, time::Duration};

use anyhow::Context as _;
use argon2::{Argon2, PasswordHasher as _, password_hash::SaltString};
use clap::{Parser, Subcommand};
use intent_db::{Db, PoolLimits};
use intent_domain::limits::NOTIFY_MAX_ATTEMPTS;
use intent_server::{config::Config, reconcile, serve, telemetry};

#[derive(Parser)]
#[command(
    about = "Intent Studio's server",
    long_about = "Intent Studio's server. Configuration comes from INTENT_* environment variables \
                  (deploy/intent-studio.env.example lists every one); docs/OPERATOR_GUIDE.md \
                  covers install, first run, upgrade, backup and troubleshooting.",
    version,
    after_help = "Examples:\n  \
                  intent-server check-config     report every configuration problem, start nothing\n  \
                  intent-server migrate          apply database migrations\n  \
                  intent-server                  serve (the same as `intent-server serve`)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve HTTP (the default). Configuration comes from `INTENT_*` variables.
    Serve,
    /// Check the configuration as `serve` would — every variable, the content store (writable),
    /// the asset worker (present), the workspace directory — then exit: 0 when it would start,
    /// 1 with every problem listed when it would not.
    CheckConfig,
    /// Apply database migrations (`INTENT_DATABASE_URL`).
    Migrate,
    /// Create the demo tenant and sites 0..N (ids 00000000-0000-4000-9000-<N in hex>).
    Seed {
        /// How many sites.
        #[arg(long, default_value_t = 10)]
        sites: u32,
    },
    /// Read a password from standard input and print its argon2id hash for
    /// `INTENT_OPERATOR_PASSWORD_HASH`.
    HashPassword,
    /// Apply the process hardening, print the dumpable flag, and exit — the
    /// shipped-binary proof that `main`'s hardening ran (Linux only). A
    /// subcommand, not an env var: an `INTENT_PROBE_DUMPABLE` left in a unit's
    /// `EnvironmentFile` would turn `serve` into a no-op `exit(0)` `systemd`
    /// counts as success (NEW-5).
    #[cfg(target_os = "linux")]
    ProbeDumpable,
    /// Check a run's acknowledgments against the database; exits 1 on any violation.
    Reconcile {
        /// The run's submission-id prefix.
        #[arg(long)]
        prefix: String,
        /// The file of `ACK <submission_id> <lead_id> <status>` lines (k6 --console-output).
        #[arg(long)]
        acks: PathBuf,
        /// Also require every outbox row to be delivered or dead after its last attempt.
        #[arg(long)]
        require_terminal: bool,
    },
}

fn database() -> anyhow::Result<Db> {
    let url = std::env::var("INTENT_DATABASE_URL").context("INTENT_DATABASE_URL is required")?;
    let limits = PoolLimits {
        max_connections: 4,
        acquire_timeout: Duration::from_secs(10),
        statement_timeout: Duration::from_secs(300),
    };
    Db::connect_lazy(&url, limits)
        .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))
}

/// The operator password's floor in characters; the ceilings match sign-in's own checks so a
/// hashed password can never be one sign-in would refuse before comparing.
const MIN_PASSWORD_CHARS: usize = 12;
const MAX_PASSWORD_BYTES: usize = 1024;

fn check_operator_password(password: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!password.is_empty(), "the password is empty");
    anyhow::ensure!(
        password.chars().count() >= MIN_PASSWORD_CHARS,
        "the password is shorter than {MIN_PASSWORD_CHARS} characters"
    );
    anyhow::ensure!(
        password.len() <= MAX_PASSWORD_BYTES,
        "the password is longer than {MAX_PASSWORD_BYTES} bytes"
    );
    Ok(())
}

/// On Linux the asset worker runs as the same user, and a dumpable process hands every
/// same-uid reader its `/proc/<pid>/environ` — the database URL, the operator hash — plus its
/// ptrace and `/proc/<pid>/mem` surface. Going non-dumpable closes all of it to anyone but
/// root (D-011). A failure means the guarantee did not take; refuse to run.
#[cfg(target_os = "linux")]
fn deny_dumpable() -> anyhow::Result<()> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .context("prctl(PR_SET_DUMPABLE)")
}

/// Process-level hardening that must hold before the first byte is read.
#[cfg(target_os = "linux")]
fn harden_process() -> anyhow::Result<()> {
    deny_dumpable()
}

/// `intent-server` — the deployed form: config, telemetry, then the server.
async fn run_server() -> anyhow::Result<()> {
    let config = Config::from_env().context("loading configuration")?;
    telemetry::init(config.log_format, &config.log_filter).context("starting telemetry")?;
    #[cfg(feature = "mutation-seams")]
    if let Some(mutation) = config.mutation {
        tracing::warn!(
            ?mutation,
            "INTENT_MUTATION is active: a protection is switched off — this build must never ship"
        );
    }
    serve::run(config).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    harden_process()?;
    match Cli::parse().command.unwrap_or(Command::Serve) {
        Command::Serve => run_server().await?,
        #[cfg(target_os = "linux")]
        Command::ProbeDumpable => {
            println!("{:?}", rustix::process::dumpable_behavior()?);
        }
        Command::CheckConfig => {
            let config = match Config::from_env() {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("configuration refused: {} problem(s)", error.problems.len());
                    for problem in &error.problems {
                        eprintln!("  - {problem}");
                    }
                    std::process::exit(1);
                }
            };
            if let Some(dir) = &config.workspace_dir
                && !dir.join("200.html").is_file()
            {
                eprintln!(
                    "configuration refused: INTENT_WORKSPACE_DIR {} has no 200.html (build the workspace app)",
                    dir.display()
                );
                std::process::exit(1);
            }
            if let Err(error) = intent_server::serve::check_studio(&config) {
                eprintln!("configuration refused: {error:#}");
                std::process::exit(1);
            }
            println!(
                "configuration ok: profile {:?}, listening on {}, content store {}, workspace {}",
                config.profile,
                config.bind,
                if config.studio.store_dir.is_some() {
                    "on"
                } else {
                    "off (uploads answer 503)"
                },
                if config.workspace_dir.is_some() {
                    "served"
                } else {
                    "not served"
                },
            );
        }
        Command::Migrate => {
            database()?
                .migrate()
                .await
                .map_err(|e| anyhow::anyhow!(e.summary()))?;
            eprintln!("migrations applied");
        }
        Command::Seed { sites } => {
            let db = database()?;
            db.seed_demo(sites)
                .await
                .map_err(|e| anyhow::anyhow!(e.summary()))?;
            eprintln!("demo tenant and {sites} site(s) present");
        }
        Command::HashPassword => {
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .context("reading the password")?;
            let password = line.trim_end_matches(['\r', '\n']);
            check_operator_password(password)?;
            // 16 salt bytes from the operating system's CSPRNG (a v4 UUID).
            let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
                .map_err(|_| anyhow::anyhow!("encoding the salt failed"))?;
            let hash = Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .map_err(|_| anyhow::anyhow!("hashing failed"))?;
            println!("{hash}");
        }
        Command::Reconcile {
            prefix,
            acks,
            require_terminal,
        } => {
            let text = std::fs::read_to_string(&acks)
                .with_context(|| format!("reading {}", acks.display()))?;
            let acks = reconcile::parse_acks(&text).map_err(anyhow::Error::msg)?;
            let report = database()?
                .reconcile_prefix(&prefix)
                .await
                .map_err(|e| anyhow::anyhow!(e.summary()))?;
            let verdict = reconcile::judge(&acks, &report, require_terminal, NOTIFY_MAX_ATTEMPTS);
            println!("{}", serde_json::to_string_pretty(&verdict)?);
            if !verdict.violations.is_empty() {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn the_process_goes_non_dumpable() {
        // There is no `Dumpable:` line in /proc/self/status — the flag answers only to
        // PR_GET_DUMPABLE.
        super::deny_dumpable().unwrap();
        assert_eq!(
            rustix::process::dumpable_behavior().unwrap(),
            rustix::process::DumpableBehavior::NotDumpable,
            "the flag did not go non-dumpable"
        );
    }

    #[test]
    fn the_operator_password_has_a_floor_and_a_ceiling() {
        use super::{MAX_PASSWORD_BYTES, MIN_PASSWORD_CHARS, check_operator_password};
        assert!(check_operator_password("").is_err());
        assert!(
            check_operator_password(&"x".repeat(MIN_PASSWORD_CHARS - 1)).is_err(),
            "an eleven-character password was accepted"
        );
        assert!(check_operator_password(&"x".repeat(MIN_PASSWORD_CHARS)).is_ok());
        // Twelve characters are twelve characters in any script.
        assert!(check_operator_password(&"é".repeat(MIN_PASSWORD_CHARS)).is_ok());
        assert!(check_operator_password(&"x".repeat(MAX_PASSWORD_BYTES)).is_ok());
        assert!(check_operator_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).is_err());
    }
}
