// FreeDSP Studio: an EQ editor for USB-C DSP cables and dongles.
//
// The device side (hid*, dsp*) talks to the hardware; eq, fr, fit and treble are the maths the
// editor runs on; store keeps things between runs; the rest is the egui front end.

// device protocols and transports
pub mod dsp;
pub mod dsp2;
pub mod dsp_fiio;
pub mod dsp_fosi;
pub mod dsp_kt;
pub mod hid;
mod hid_api;
mod hid_desc;
mod hid_fiio;
mod hid_gen2;
mod hid_report;
#[cfg(windows)]
mod hid_win;
#[cfg(not(windows))]
mod hid_hidapi;

// the editor's maths
pub mod eq;
pub mod fit;
pub mod fr;
pub mod treble;

pub mod store;

// the app
mod app;
mod audio;
mod device;
mod dialogs;
mod theme;
mod ui;

pub use app::run;
