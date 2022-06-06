use crate::Config;
use chrono::Local;
use fern::{
  colors::{Color, ColoredLevelConfig},
  Dispatch,
};
use log::LevelFilter;
use serde::Deserialize;
#[derive(Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
  Error,
  Warn,
  Info,
  Debug,
  Trace,
}
impl Default for LogLevel {
  fn default() -> Self {
    Self::Debug
  }
}
impl From<LogLevel> for LevelFilter {
  fn from(level: LogLevel) -> Self {
    match level {
      LogLevel::Error => LevelFilter::Error,
      LogLevel::Warn => LevelFilter::Warn,
      LogLevel::Info => LevelFilter::Info,
      LogLevel::Debug => LevelFilter::Debug,
      LogLevel::Trace => LevelFilter::Trace,
    }
  }
}
pub fn setup_logging(config: &Config) -> anyhow::Result<()> {
  let log_timestamps = config.log_timestamps;
  let colors = ColoredLevelConfig::new()
    .info(Color::Green)
    .debug(Color::Magenta)
    .warn(Color::Yellow)
    .error(Color::Red);
  let mut dispatch = Dispatch::new()
    .format(move |out, msg, record| {
      let mut target = record.target().to_string();

      target = format!("[{}] ", target);
      if log_timestamps {
        out.finish(format_args!(
          "[{}] {: >5} {}{}",
          Local::now().format("%y/%m/%d %H:%M:%S%.3f"),
          colors.color(record.level()),
          target,
          msg
        ))
      } else {
        out.finish(format_args!("{: >5} {}{}", record.level(), target, msg))
      }
    })
    .level(config.log_level.into())
    .level_for("tracing", LevelFilter::Warn)
    .level_for("async_tungstenite", LevelFilter::Debug)
    .chain(std::io::stdout());
  if config.log_level != LogLevel::Trace {
    dispatch = dispatch
      .level_for("serenity", LevelFilter::Warn)
      .level_for("h2", LevelFilter::Warn)
      .level_for("hyper", LevelFilter::Warn)
      .level_for("rustls", LevelFilter::Warn)
      .level_for("reqwest", LevelFilter::Warn)
      .level_for("tungstenite", LevelFilter::Warn);
  }
  dispatch.apply()?;
  Ok(())
}
