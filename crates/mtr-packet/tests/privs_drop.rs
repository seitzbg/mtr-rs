//! `privs::drop_all()` mutates the whole process (uid, gid, capability sets), so it lives in
//! its own test binary with exactly one test: inside the crate's unit-test binary it would
//! race the raw-socket tests of Tasks 7 and 10-15, which run as threads of the same process
//! and would see `cap_net_raw` vanish mid-run once the binary is given the capability.
//! Do not add a second test to this file. GPL-2.0-only.

#[test]
fn dropping_leaves_no_capabilities_and_a_consistent_uid() {
    mtr_packet::privs::drop_all().unwrap();
    for set in [
        caps::CapSet::Effective,
        caps::CapSet::Permitted,
        caps::CapSet::Inheritable,
    ] {
        assert!(caps::read(None, set).unwrap().is_empty(), "{set:?}");
    }
    assert_eq!(nix::unistd::geteuid(), nix::unistd::getuid());
    assert_eq!(nix::unistd::getegid(), nix::unistd::getgid());
    // Idempotent: a second call is also fine.
    mtr_packet::privs::drop_all().unwrap();
}
