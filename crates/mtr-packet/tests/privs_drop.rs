//! `privs::drop_all()` mutates the whole process (uid, gid, capability sets), so it lives in
//! its own test binary with exactly one test: inside the crate's unit-test binary it would
//! race the raw-socket tests of Tasks 7 and 10-15, which run as threads of the same process
//! and would see `cap_net_raw` vanish mid-run once the binary is given the capability.
//! Do not add a second test to this file. Linux only: the capability sets it inspects do not
//! exist on FreeBSD or macOS, where `drop_all()` is the plain `setuid()` drop. GPL-2.0-only.
#![cfg(target_os = "linux")]

#[test]
fn dropping_leaves_only_a_granted_net_admin_and_a_consistent_uid() {
    // Deviation 34: `CAP_NET_ADMIN` survives the drop iff it was effective beforehand, so
    // `SO_MARK` keeps working when the helper was given `cap_net_admin+ep`.
    let before = mtr_packet::privs::has_net_admin();
    mtr_packet::privs::drop_all().unwrap();
    let expected: caps::CapsHashSet = if before {
        [caps::Capability::CAP_NET_ADMIN].into_iter().collect()
    } else {
        caps::CapsHashSet::new()
    };
    for set in [caps::CapSet::Effective, caps::CapSet::Permitted] {
        assert_eq!(caps::read(None, set).unwrap(), expected, "{set:?}");
    }
    assert!(
        caps::read(None, caps::CapSet::Inheritable)
            .unwrap()
            .is_empty()
    );
    assert_eq!(mtr_packet::privs::has_net_admin(), before);
    assert_eq!(nix::unistd::geteuid(), nix::unistd::getuid());
    assert_eq!(nix::unistd::getegid(), nix::unistd::getgid());
    // Idempotent: a second call is also fine.
    mtr_packet::privs::drop_all().unwrap();
}
