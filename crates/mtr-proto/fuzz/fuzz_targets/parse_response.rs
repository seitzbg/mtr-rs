//! GPL-2.0-only.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(r) = mtr_proto::Response::parse(s) {
            let again = mtr_proto::Response::parse(&r.encode()).expect("re-parse of encoded response");
            assert_eq!(again, r);
        }
    }
});
