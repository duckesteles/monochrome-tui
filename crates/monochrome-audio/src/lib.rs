pub mod cenc;
pub mod convert;
pub mod engine;
pub mod probe;
pub mod ring;
pub mod source;
pub mod spill;

pub use engine::{Command, Event, PlayRequest, Player};

pub fn use_ring_for_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
