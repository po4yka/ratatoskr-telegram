//! Closed GitHub repository action names used by callback token rows.

use ratatoskr_github_contracts::RepositoryActionCapability;

const fn repository_mode_name(mode: RepositoryActionCapability) -> &'static str {
    match mode {
        RepositoryActionCapability::Metadata => "metadata",
        RepositoryActionCapability::Track => "track",
        RepositoryActionCapability::Star => "star",
        _ => "unsupported",
    }
}

pub(super) fn repository_mode(value: &str) -> Option<RepositoryActionCapability> {
    match value {
        "metadata" => Some(RepositoryActionCapability::Metadata),
        "track" => Some(RepositoryActionCapability::Track),
        "star" => Some(RepositoryActionCapability::Star),
        _ => None,
    }
}

pub(super) fn selection_action(mode: RepositoryActionCapability) -> String {
    format!("select_{}", repository_mode_name(mode))
}
