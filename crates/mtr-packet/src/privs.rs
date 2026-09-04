//! Drop setuid privileges and every capability once the sockets are open. Ported from
//! packet/packet.c:43-102 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use caps::CapSet;
use nix::unistd::{getegid, geteuid, getgid, getuid, setgid, setuid};

use crate::Fatal;

/// `drop_elevated_permissions()`: `setgid(getgid())`, `setuid(getuid())`, verify, then clear
/// the effective, permitted and inheritable sets (`cap_clear` + `cap_set_proc`) so nothing
/// after this point can regain privilege.
///
/// Deviation 34: keep `CAP_NET_ADMIN` — and only that — when the file capability granted it, so
/// `SO_MARK` (the `--mark` option) can work after the drop. C clears everything and lets
/// `setsockopt(SO_MARK)` fail later, which is why `-M` is silently broken there.
pub fn drop_all() -> Result<(), Fatal> {
    let perm = |e: nix::Error| Fatal::Message(format!("Unable to drop elevated permissions: {e}"));
    setgid(getgid()).map_err(perm)?;
    setuid(getuid()).map_err(perm)?;
    if geteuid() != getuid() || getegid() != getgid() {
        return Err(Fatal::Message("Unable to drop elevated permissions".into()));
    }
    // Computed *after* the setuid: dropping from root to a non-zero uid already clears the
    // capability sets, so this asks what we actually still hold.
    let keep_net_admin = has_net_admin();
    let cap_err =
        |e: caps::errors::CapsError| Fatal::Message(format!("Failed to drop capabilities: {e}"));
    caps::clear(None, CapSet::Inheritable).map_err(cap_err)?;
    let mut keep = caps::CapsHashSet::new();
    if keep_net_admin {
        keep.insert(caps::Capability::CAP_NET_ADMIN);
    }
    // Permitted is the ceiling for Effective: shrink Permitted first, then set Effective to the
    // same set, so neither call is ever asked to raise a capability we no longer hold.
    caps::set(None, CapSet::Permitted, &keep).map_err(cap_err)?;
    caps::set(None, CapSet::Effective, &keep).map_err(cap_err)?;
    Ok(())
}

/// Whether `SO_MARK` will be accepted by the kernel for this process, i.e. whether `mark` can
/// honestly be reported as supported.
pub fn has_net_admin() -> bool {
    caps::has_cap(None, CapSet::Effective, caps::Capability::CAP_NET_ADMIN).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn has_net_admin_is_false_without_the_capability() {
        // The unit-test binary is never setcap'd; if it were, this test is meaningless and
        // the MTR_E2E privs_drop test covers it.
        if caps::has_cap(
            None,
            caps::CapSet::Effective,
            caps::Capability::CAP_NET_ADMIN,
        )
        .unwrap_or(false)
        {
            eprintln!("skipped: test binary holds CAP_NET_ADMIN");
            return;
        }
        assert!(!super::has_net_admin());
    }
}
