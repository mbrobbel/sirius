//! Versioned metadata carried by native Substrait exchange boundaries.

include!(concat!(env!("OUT_DIR"), "/sirius.exchange.v1.rs"));

pub const SOURCE_TYPE_URL: &str = "type.googleapis.com/sirius.exchange.v1.ExchangeSource";
pub const SINK_TYPE_URL: &str = "type.googleapis.com/sirius.exchange.v1.ExchangeSink";
pub const DESTINATION_TYPE_URL: &str = "type.googleapis.com/sirius.exchange.v1.ExchangeDestination";
