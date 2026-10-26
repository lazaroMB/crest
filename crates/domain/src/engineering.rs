//! Engineering viewers: digital waveforms (`show_waveform`), netlists as
//! connectivity (`show_netlist`), material data cards (`show_material`),
//! STEP parts in the 3D view (`show_step`), 2D DXF drawings (`show_dxf`)
//! and IFC room schedules (`show_ifc_schedule`).
//!
//! Adapters parse the files (VCD, FST and GHW dumps; SPICE decks and
//! KiCad netlists; STEP, DXF and IFC) into these specs, which the UI draws as they are. The
//! helpers here (values at a time, radix formatting, clock periods, nets
//! with one pin) are what the tools summarise for the model. The
//! engineering calculators (Bode plots, Karnaugh maps, beams, tolerance
//! stacks, BOM costing) are computed cards, under [`crate::computed`].

pub mod cad;
pub mod drawing;
pub mod ifc;
pub mod material;
pub mod netlist;
pub mod waveform;

use crate::block::BlockError;

/// A [`BlockError::Invalid`] of block `kind`.
pub(crate) fn invalid(kind: &'static str, reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind,
        reason: reason.into(),
    }
}
