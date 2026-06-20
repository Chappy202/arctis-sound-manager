pub mod error;
pub mod schema;

pub use error::ConfigError;
pub use schema::{ChannelConfig, Config, EqBandConfig, Profile, RouteConfig, CURRENT_VERSION};
