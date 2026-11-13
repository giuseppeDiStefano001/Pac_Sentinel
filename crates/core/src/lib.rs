//! Dominio puro per la gestione del portafoglio PAC a lungo termine.
//! Nessun I/O, nessun accesso alla rete o al filesystem, nessun uso di `f64` per valori monetari o quote.

pub mod config;
pub mod error;
pub mod pac;
pub mod portfolio;
pub mod signals;
pub mod simulator;
pub mod tax;
pub mod types;

pub const LEGAL_DISCLAIMER: &str =
    "Strumento di supporto basato su regole personali, non è consulenza finanziaria.";