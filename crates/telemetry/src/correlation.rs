//! Correlation: reading the trace context a span carries.
//!
//! Correlation minting — the namespaced `correlation:` identifier every unit of work receives —
//! arrives with the first wire interaction, together with the contracts types it must be one of.

use opentelemetry::trace::TraceContextExt as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

/// The W3C trace id of `span`, when it carries a valid one.
///
/// Spans created before a subscriber with an OpenTelemetry layer is installed, and spans outside
/// any traced operation, carry no valid context; both are `None` rather than an all-zero id, so a
/// caller can distinguish "not traced" from "traced with a broken id".
#[must_use]
pub fn trace_id_of(span: &tracing::Span) -> Option<opentelemetry::trace::TraceId> {
    let context = span.context();
    let span_context = context.span().span_context().clone();
    if !span_context.is_valid() {
        return None;
    }
    Some(span_context.trace_id())
}
