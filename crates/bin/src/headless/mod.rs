//! Headless mode: one turn in the terminal, without UI.

pub mod approval;
pub mod sink;
pub mod terminal;

pub use approval::StdinApproval;
pub use sink::HeadlessSink;
pub use terminal::Terminal;
