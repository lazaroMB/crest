//! Headless mode: one turn, the research flow of a problem, the
//! implementation flow or the simulation of a Spec, in the terminal,
//! without UI.

pub mod approval;
pub mod implement;
pub mod interrupt;
pub mod research;
pub mod run;
pub mod simulate;
pub mod sink;
pub mod terminal;

pub use approval::StdinApproval;
pub use run::{Exit, run, run_turn};
pub use sink::HeadlessSink;
pub use terminal::Terminal;
