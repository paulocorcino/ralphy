//! `ralphy host`: make a computer reached over SSH a peer, check it, and take
//! it out again (ADR-0067). Sign-in uses a key or an agent only; Ralphy never
//! prompts.

#![allow(dead_code)]

mod checks;
mod pair;
mod shell;
mod ssh;
