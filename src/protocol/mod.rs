//! Reactotron wire-protocol types and codec.
//!
//! Public surface:
//!
//! - [`Message`] — the full tagged-union of frames rustotron handles.
//! - [`encode`] / [`decode`] — round-trip between a JSON text frame and
//!   a [`Message`].
//! - [`CodecError`] — the narrow set of hard failures; most degradations
//!   land as [`Message::Unknown`] per NFR-9.
//!
//! Payload structs (`ClientIntroPayload`, `ApiResponsePayload`, …) are
//! re-exported for the store / server / TUI layers to consume directly.
//!
//! See `docs/protocol.md` for the wire-level reference and
//! `docs/decisions/003-protocol-reality.md` for the mental model.

pub mod api_events;
pub mod body;
pub mod handshake;
pub mod messages;
pub mod repair;

pub use api_events::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
pub use body::{Body, MAX_STORED_BODY_BYTES};
pub use handshake::ClientIntroPayload;
pub use messages::{CodecError, Envelope, Message, decode, encode};
