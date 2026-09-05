//! GPL-2.0-only.
#![no_main]
use libfuzzer_sys::fuzz_target;
use mtr_packet::backend::unix::deconstruct::decode_mpls;

fuzz_target!(|data: &[u8]| {
    let labels = decode_mpls(data);
    assert!(labels.len() <= 8);
});
