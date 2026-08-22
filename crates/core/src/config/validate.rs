//! The startup validation rules V1–V7 and the operator-facing failure report.
//!
//! Order at startup is strictly: extract, validate, initialise telemetry, bind listeners. Telemetry
//! is initialised *after* validation so that an invalid `log_filter` fails as a configuration
//! problem on stderr rather than inside subscriber setup, where nothing could report it.
//!
//! figment's own extraction is fail-fast, so the "report every problem" guarantee comes from this
//! pass and not from serde.

use std::fmt::Write as _;

use figment::error::Kind;
use tracing_subscriber::EnvFilter;

use secrecy::ExposeSecret;

use crate::config::model::TelegramConfig;
use crate::role::RuntimeRole;

/// One startup-rule violation.
///
/// Every member is `&'static str`. It is therefore STRUCTURALLY IMPOSSIBLE for a supplied value to
/// appear in a configuration failure report, so the report can never echo a secret. This is a type
/// property, not a rule someone has to remember.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The dotted configuration path, e.g. `database.url`.
    pub key: &'static str,
    /// The environment variable that sets it, e.g. `RATATOSKR__DATABASE__URL`.
    pub env_var: &'static str,
    /// What the rule requires.
    pub rule: &'static str,
}

/// The ceiling V3 puts on `drain_seconds + grace_seconds`: a total above it guarantees SIGKILL
/// mid-drain under systemd's default 90-second stop timeout.
pub const SHUTDOWN_CEILING_SECONDS: u64 = 120;

/// Applies V1–V7 and returns every violation found, in rule order.
pub(crate) fn validate(role: RuntimeRole, config: &TelegramConfig) -> Vec<Violation> {
    let _ = role;
    let mut found = Vec::new();

    // V1 — a bad filter otherwise silences every log line at the moment you need them.
    if EnvFilter::try_new(&config.telemetry.log_filter).is_err() {
        found.push(Violation {
            key: "telemetry.log_filter",
            env_var: "RATATOSKR__TELEMETRY__LOG_FILTER",
            rule: "must parse as a tracing-subscriber EnvFilter directive string, e.g. info",
        });
    }

    // V2 and V3 — the shutdown windows. A drain past 60 seconds outraces no supervisor, and a total
    // above the ceiling guarantees SIGKILL mid-request under a default stop timeout.
    let drain = config.shutdown.drain_seconds;
    let grace = config.shutdown.grace_seconds;
    if drain > 60 {
        found.push(Violation {
            key: "shutdown.drain_seconds",
            env_var: "RATATOSKR__SHUTDOWN__DRAIN_SECONDS",
            rule: "must be 0..=60, and drain_seconds + grace_seconds must not exceed 120",
        });
    }
    if !(1..=SHUTDOWN_CEILING_SECONDS).contains(&grace)
        || drain.saturating_add(grace) > SHUTDOWN_CEILING_SECONDS
    {
        found.push(Violation {
            key: "shutdown.grace_seconds",
            env_var: "RATATOSKR__SHUTDOWN__GRACE_SECONDS",
            rule: "must be 1..=120, and drain_seconds + grace_seconds must not exceed 120",
        });
    }

    found.extend(otlp_violations(config));
    found.extend(database_violations(config));

    found
}

/// V4 to V7 — the OTLP exporter rules. One subsystem, one function, so [`validate`] stays inside
/// the workspace's function-length lint.
fn otlp_violations(config: &TelegramConfig) -> Vec<Violation> {
    let mut found = Vec::new();
    let Some(otlp) = config.telemetry.otlp.as_ref() else {
        return found;
    };

    // V4 — the scheme. `https` everywhere except a loopback collector, which is what a developer
    // runs without TLS on their own machine.
    let loopback = otlp.endpoint.host().is_some_and(|host| {
        matches!(host, url::Host::Ipv4(ip) if ip.is_loopback())
            || matches!(host, url::Host::Domain(domain) if domain == "localhost")
    });
    let scheme = otlp.endpoint.scheme();
    if otlp.endpoint.host().is_none()
        || !matches!(scheme, "http" | "https")
        || (scheme == "http" && !loopback)
    {
        found.push(Violation {
            key: "telemetry.otlp.endpoint",
            env_var: "RATATOSKR__TELEMETRY__OTLP__ENDPOINT",
            rule: "must be an https URL with a host; http is accepted only for a loopback \
                   collector",
        });
    }

    // V5
    if !(1..=60).contains(&otlp.timeout_seconds) {
        found.push(Violation {
            key: "telemetry.otlp.timeout_seconds",
            env_var: "RATATOSKR__TELEMETRY__OTLP__TIMEOUT_SECONDS",
            rule: "must be 1..=60",
        });
    }

    // V6 — a header name containing a control character is a request-splitting primitive.
    if !otlp.headers.keys().all(|name| is_header_name(name)) {
        found.push(Violation {
            key: "telemetry.otlp.headers",
            env_var: "RATATOSKR__TELEMETRY__OTLP__HEADERS__<NAME>",
            rule: "every header name must match ^[a-z0-9-]{1,64}$",
        });
    }

    // V7 — `Url` is the second credential carrier in this tree, and the only one that is not a
    // `SecretString`: its `Debug` prints user information as plain fields, and the whole
    // configuration is rendered with `Debug` into the effective-configuration log line and into
    // check-config's output. The header map is the one place a collector credential may live.
    if !otlp.endpoint.username().is_empty()
        || otlp.endpoint.password().is_some()
        || otlp.endpoint.query().is_some()
    {
        found.push(Violation {
            key: "telemetry.otlp.endpoint",
            env_var: "RATATOSKR__TELEMETRY__OTLP__ENDPOINT",
            rule: "must not embed a user name, a password or a query string; a collector \
                   credential belongs in telemetry.otlp.headers, which cannot be printed",
        });
    }

    found
}

/// V8 — the database rules. Extracted so [`validate`] stays inside the workspace's function-length
/// lint, along a boundary that means something.
fn database_violations(config: &TelegramConfig) -> Vec<Violation> {
    let mut found = Vec::new();
    let Some(database) = config.database.as_ref() else {
        return found;
    };

    // Pool bounds. Zero reads as "off" to whoever typed it and behaves as "refuse everything" in
    // the code; there is no spelling of that misunderstanding this service wants to serve.
    if !(1..=100).contains(&database.max_connections) {
        found.push(Violation {
            key: "database.max_connections",
            env_var: "RATATOSKR__DATABASE__MAX_CONNECTIONS",
            rule: "must be 1..=100; zero refuses every connection rather than disabling the limit",
        });
    }

    if !(1..=30).contains(&database.acquire_timeout_seconds) {
        found.push(Violation {
            key: "database.acquire_timeout_seconds",
            env_var: "RATATOSKR__DATABASE__ACQUIRE_TIMEOUT_SECONDS",
            rule: "must be 1..=30",
        });
    }

    // The scheme. `postgres://` and `postgresql://` are the two sqlx accepts; anything else fails
    // at connect time, which is after the process has reported itself started. A configuration
    // error must be a startup error.
    let url = database.url.expose_secret();
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        found.push(Violation {
            key: "database.url",
            env_var: "RATATOSKR__DATABASE__URL",
            rule: "must be a postgres:// or postgresql:// URL",
        });
    }

    found
}

/// `^[a-z0-9-]{1,64}$`, spelled without a regular-expression dependency.
fn is_header_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The operator-facing report for a set of violations. One block per problem, stable order, no
/// supplied values.
pub(crate) fn report_invalid(role: RuntimeRole, violations: &[Violation]) -> String {
    let plural = if violations.len() == 1 { "" } else { "s" };
    let mut out = format!(
        "{}: refusing to start; {} configuration problem{plural}.\n\n",
        role.binary_name(),
        violations.len(),
    );
    for violation in violations {
        let _ = writeln!(
            out,
            "  {}\n      {}\n      {}\n",
            violation.key, violation.env_var, violation.rule
        );
    }
    push_footer(&mut out, role);
    out
}

/// The operator-facing report for an extraction failure.
///
/// figment's message is deliberately NOT interpolated: it can quote the supplied value, and a
/// configuration report that echoes a value can echo a secret. Only keys are named.
pub(crate) fn report_unreadable(role: RuntimeRole, error: &figment::Error) -> String {
    let mut out = format!(
        "{}: refusing to start; the configuration could not be read.\n\n",
        role.binary_name(),
    );
    for problem in error.clone() {
        let key = key_of(&problem);
        let _ = writeln!(
            out,
            "  {key}\n      {}\n      {}\n",
            env_var_of(&key),
            reason_of(&problem),
        );
    }
    push_footer(&mut out, role);
    out
}

/// The two closing lines every report ends with.
fn push_footer(out: &mut String, role: RuntimeRole) {
    let _ = write!(
        out,
        "Supplied values are never echoed.\nValidate without starting: {} check-config\n",
        role.binary_name(),
    );
}

/// The dotted key an extraction failure is about; keys are safe to print, values are not.
fn key_of(error: &figment::Error) -> String {
    let path = error.path.join(".");
    match &error.kind {
        // figment reports a missing member under its PARENT's path, so the path alone names a key
        // the operator supplied correctly and an environment variable that cannot set the missing
        // field. Appending the member's own name is what makes the block actionable.
        Kind::MissingField(name) if path.is_empty() => name.to_string(),
        Kind::MissingField(name) => format!("{path}.{name}"),
        _ if !path.is_empty() => path,
        Kind::UnknownField(name, _) => name.clone(),
        _ => "(the provider did not report a key)".to_owned(),
    }
}

/// The environment variable a dotted key is set by.
fn env_var_of(key: &str) -> String {
    format!("RATATOSKR__{}", key.replace('.', "__").to_uppercase())
}

/// What went wrong, in terms that never quote the supplied value.
const fn reason_of(error: &figment::Error) -> &'static str {
    match &error.kind {
        Kind::UnknownField(_, _) => "is not a configuration key of this process",
        Kind::MissingField(_) => "is required and was not supplied",
        _ => "could not be read as the type of this field",
    }
}
