//! Headless mode: one turn in the terminal, without UI.

pub mod sink;
pub mod terminal;

pub use sink::HeadlessSink;
pub use terminal::Terminal;
