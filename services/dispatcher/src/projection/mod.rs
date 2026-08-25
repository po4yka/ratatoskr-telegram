//! The operation projection: typed contract input, the escaping renderer, and the consumer whose
//! accept step guards every render into the durable queue.
//!
//! Each module owns one stage: [`event`] parses the published envelope down to what rendering
//! needs, [`render`] turns an event into deterministic escaped Telegram HTML, and [`consumer`]
//! drives the transactional accept that deduplicates, orders, throttles, and enqueues.

pub mod consumer;
pub mod event;
pub mod render;

pub use crate::projection::consumer::{AcceptOutcome, ProjectionConsumer};
pub use crate::projection::event::{
    OperationEvent, OperationStatus, ParseError, SafeLine, from_envelope_json,
};
pub use crate::projection::render::render;
