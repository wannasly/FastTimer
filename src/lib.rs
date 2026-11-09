pub mod benchmark;
pub mod timer;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
