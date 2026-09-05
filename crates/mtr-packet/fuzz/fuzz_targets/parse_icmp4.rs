//! GPL-2.0-only.
#![no_main]
use libfuzzer_sys::fuzz_target;
use mtr_packet::backend::unix::deconstruct::parse_icmp4;

fuzz_target!(|data: &[u8]| {
    let Some((&flag, packet)) = data.split_first() else { return };
    // Must never panic; the result itself is unconstrained.
    let _ = parse_icmp4(packet, flag & 1 == 1);
});
