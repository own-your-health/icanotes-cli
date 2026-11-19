//! `icanotes`: a patient's own records from a practice on ICANotes, through
//! the patient API key the practice's patient portal issues
//! (patientonlineportal.com), as the `health-records/v1` profile
//! (medical-common). `docs/api.md` has the API, its limits and its traps.

pub mod ccd;
pub mod cli;
pub mod client;
pub mod commands;
pub mod config;
pub mod credential;
pub mod export;
pub mod login;
pub mod ratelimit;
pub mod redact;
pub mod zip;
