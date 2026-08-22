//! The error taxonomy: subsystem labels, the internal arm, and what never leaks.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use telegram_core::error::{Subsystem, TelegramError};

/// A stand-in dependency failure, as any crate's concrete error would arrive.
#[derive(Debug, thiserror::Error)]
#[error("the database connection could not be established")]
struct DependencyFailure;

/// Every subsystem renders its stable lowercase label — the value telemetry uses.
#[test]
fn every_subsystem_renders_its_stable_label() {
    let labels: &[(Subsystem, &str)] = &[
        (Subsystem::Config, "config"),
        (Subsystem::Telemetry, "telemetry"),
        (Subsystem::Http, "http"),
        (Subsystem::Persistence, "persistence"),
    ];
    for (subsystem, label) in labels {
        assert_eq!(subsystem.to_string(), *label);
    }
}

/// Any dependency failure becomes an internal failure carrying its subsystem and its source.
#[test]
fn a_dependency_failure_is_an_internal_failure_of_its_subsystem() {
    let error = TelegramError::internal(Subsystem::Persistence, DependencyFailure);
    match &error {
        TelegramError::Internal { subsystem, .. } => {
            assert_eq!(*subsystem, Subsystem::Persistence);
        }
        // The enum is non_exhaustive by contract, so the arm is mandatory; it is unreachable while
        // the internal arm is the only one, and a new variant must answer for itself here.
        _ => panic!("unexpected variant"),
    }
}

/// `Display` carries no diagnostics; the source chain is where they live. A boundary logs once from
/// `source()`, and nothing that renders the error for a caller ever reads it.
#[test]
fn display_carries_no_diagnostics() {
    let error = TelegramError::internal(Subsystem::Persistence, DependencyFailure);
    let rendered = error.to_string();
    assert!(
        !rendered.contains("the database connection"),
        "Display leaked the source chain: {rendered}",
    );
}
