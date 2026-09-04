//! GPL-2.0-only.
#![no_main]
use libfuzzer_sys::fuzz_target;

// The helper's C-compatibility layer: `strtol_full` reimplements strtol's saturation and
// trailing-garbage rules by hand, and `decode_send_probe` runs every send-probe argument
// through it. Both are fed straight from the command pipe, so they see arbitrary bytes.
fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    for field in s.split('\n') {
        // Saturating, never panicking, and agreeing with Rust's own parser whenever that
        // parser accepts the same text.
        if let Some(v) = mtr_proto::cdecode::strtol_full(field) {
            if let Ok(direct) = field.trim_start().trim_start_matches('+').parse::<i64>() {
                assert_eq!(v, direct, "strtol_full disagrees on {field:?}");
            }
        }
    }
    if let Ok(line) = mtr_proto::tokenize::tokenize(s) {
        // Never panics; a decoded parameter set must survive being decoded again from the
        // same line (the decoder is pure).
        if let Ok(params) = mtr_proto::cdecode::decode_send_probe(&line) {
            let again = mtr_proto::cdecode::decode_send_probe(&line)
                .expect("decode_send_probe is deterministic");
            assert_eq!(params, again);
        }
    }
});
