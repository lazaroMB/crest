//! Headless mode: one turn in the terminal, without UI.

pub mod approval;
pub mod interrupt;
pub mod run;
pub mod sink;
pub mod terminal;

pub use approval::StdinApproval;
pub use run::{Exit, run, run_turn};
pub use sink::HeadlessSink;
pub use terminal::Terminal;
