extern crate dotenv;

use crate::logging;
use dotenv::dotenv;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct Config {
  pub log_level: logging::LogLevel,
  pub log_timestamps: bool,
  pub log_colored: bool,
}
impl Default for Config {
  fn default() -> Self {
    Self {
      log_level: Default::default(),
      log_timestamps: true,
      log_colored: true,
    }
  }
}

impl Config {
  pub fn load() -> anyhow::Result<Self> {
    dotenv().ok();

    Ok(envy::from_env::<Self>()?)
  }
}
