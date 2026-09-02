#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(r) = mtr_proto::Request::parse(s) {
            let again = mtr_proto::Request::parse(&r.encode()).expect("re-parse of encoded request");
            assert_eq!(again, r);
        }
    }
});
