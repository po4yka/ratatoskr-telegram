//! Which deployable this process is.

/// Which deployable this process is.
///
/// Fixed by the binary at compile time and never read from the environment: a role that could be
/// misconfigured would make a process lie in every metric it emits and would let an operator start
/// `ratatoskr-telegram-dispatcher` in the webhook role.
///
/// This is a DEPLOYMENT axis — separate network exposure, separate database roles, separate
/// credentials (`ratatoskr-workspace` deployment documents). It is not a wire identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeRole {
    /// `ratatoskr-telegram-webhook` — receives Bot API updates over HTTPS.
    Webhook,
    /// `ratatoskr-telegram-dispatcher` — projects operations into ordered Bot API sends.
    Dispatcher,
}

impl RuntimeRole {
    /// Every role, so the `role` telemetry label can never become unbounded. The array length is
    /// the documented count, so adding a variant without updating it does not compile.
    pub const ALL: [Self; 2] = [Self::Webhook, Self::Dispatcher];

    /// The telemetry label and health-body value: `webhook` | `dispatcher`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Dispatcher => "dispatcher",
        }
    }

    /// `ratatoskr-telegram-webhook` | `ratatoskr-telegram-dispatcher`.
    #[must_use]
    pub const fn binary_name(self) -> &'static str {
        match self {
            Self::Webhook => "ratatoskr-telegram-webhook",
            Self::Dispatcher => "ratatoskr-telegram-dispatcher",
        }
    }

    /// Distinct per role so both binaries run on one developer machine with no configuration:
    /// `9467` | `9468`. The block continues platform's 9464–9466 operator allocation and stays
    /// clear of every port the shared deployment-target document records as held.
    #[must_use]
    pub const fn default_admin_port(self) -> u16 {
        match self {
            Self::Webhook => 9467,
            Self::Dispatcher => 9468,
        }
    }
}

impl core::fmt::Display for RuntimeRole {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
