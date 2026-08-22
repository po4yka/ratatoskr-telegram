//! The role axis and its per-role facts.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use telegram_core::RuntimeRole;

/// The role set is exactly `{webhook, dispatcher}`: two deployables, both named by README.md.
#[test]
fn the_role_set_is_exactly_the_two_planned_deployables() {
    assert_eq!(RuntimeRole::ALL.len(), 2);
    assert!(RuntimeRole::ALL.contains(&RuntimeRole::Webhook));
    assert!(RuntimeRole::ALL.contains(&RuntimeRole::Dispatcher));
}

/// Labels and binary names are distinct across roles, or a shared log stream could not tell two
/// processes apart.
#[test]
fn every_role_has_distinct_labels_and_binary_names() {
    for role in RuntimeRole::ALL {
        assert_eq!(role.as_str(), role.to_string());
        assert!(role.binary_name().starts_with("ratatoskr-telegram-"));
        for other in RuntimeRole::ALL {
            if role == other {
                continue;
            }
            assert_ne!(role.as_str(), other.as_str());
            assert_ne!(role.binary_name(), other.binary_name());
        }
    }
}

/// Distinct default admin ports so both binaries run on one developer machine unconfigured, outside
/// platform's allocated 9464–9466 operator block (`DEPLOYMENT_TARGET.md`, Ports).
#[test]
fn default_admin_ports_are_distinct_and_outside_platforms_block() {
    let mut ports: Vec<u16> = RuntimeRole::ALL
        .map(RuntimeRole::default_admin_port)
        .to_vec();
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(ports.len(), 2);
    for port in ports {
        assert!(
            !(9464..=9466).contains(&port),
            "port {port} collides with platform's operator block",
        );
    }
}
