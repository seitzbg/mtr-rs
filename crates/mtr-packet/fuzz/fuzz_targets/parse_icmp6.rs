//! GPL-2.0-only.
#![no_main]
use libfuzzer_sys::fuzz_target;
use mtr_packet::backend::linux::deconstruct::parse_icmp6;

fuzz_target!(|data: &[u8]| {
    // Must never panic; the result itself is unconstrained.
    let _ = parse_icmp6(data);
});
