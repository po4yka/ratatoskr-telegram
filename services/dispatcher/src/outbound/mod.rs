//! Outbound delivery policy: the injected clock, the delivery rate limiter, and the Bot API
//! failure classifier.
//!
//! Each module owns one decision the sender loop consults and nothing else: when a call may go
//! on the wire ([`limiter`]), what a failure means for the job ([`classify`]), and where time
//! comes from ([`clock`]).

pub mod classify;
pub mod clock;
pub mod limiter;
pub mod sender;

pub use crate::outbound::classify::{Classified, PermanentClass, classify};
pub use crate::outbound::clock::{Clock, SystemClock};
pub use crate::outbound::limiter::{DeliveryLimiter, RateDecision};
pub use crate::outbound::sender::{
    AcknowledgementFuture, AcknowledgementStore, BotApiSink, ClientSink, OutboundSender,
    SenderError, SenderLimits, SentMessage,
};
