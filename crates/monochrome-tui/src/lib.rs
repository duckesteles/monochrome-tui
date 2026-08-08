pub mod app;
pub mod config;
pub mod diagnostics;
pub mod dispatch;
pub mod filter;
pub mod help;
pub mod input;
pub mod paths;
pub mod secrets;
pub mod sync;
pub mod theme;
pub mod uninstall;
pub mod views;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod view_tests;
