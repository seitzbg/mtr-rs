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
    // Ambient first, and explicitly: the kernel does drop it as a side effect of clearing
    // Inheritable (`cap_capset()` intersects ambient with inheritable ∩ permitted), but that
    // makes a security property depend on the order of the two calls below. `capset` on the
    // ambient set is `prctl(PR_CAP_AMBIENT_CLEAR_ALL)`, unsupported before Linux 4.3 and
    // inside some sandboxes, so a failure here is not fatal — the clears below still cover it.
    let _ = caps::clear(None, CapSet::Ambient);
    caps::clear(None, CapSet::Inheritable).map_err(cap_err)?;
    let mut keep = caps::CapsHashSet::new();
    if keep_net_admin {
        keep.insert(caps::Capability::CAP_NET_ADMIN);
    }
    // Effective must be a subset of Permitted in the value `capset()` receives
    // (`cap_capset()`, security/commoncap.c), and `caps::set` rewrites only the set named,
    // submitting the others unchanged. So shrink Effective first — still under the old, larger
    // Permitted — and only then shrink Permitted to match. The other order is rejected with
    // `EPERM` on every process that actually holds a capability.
    caps::set(None, CapSet::Effective, &keep).map_err(cap_err)?;
    caps::set(None, CapSet::Permitted, &keep).map_err(cap_err)?;
    Ok(())
}

/// Whether this process holds `CAP_NET_ADMIN`, which is what the kernel checks for `SO_MARK`
/// once [`drop_all`] has removed `CAP_NET_RAW` — since Linux 5.17 `sock_setsockopt()` accepts
/// `SO_MARK` under either capability, so this would be a false negative for a process that kept
/// `CAP_NET_RAW`, which ours never does. It is also optimistic inside a user namespace whose
/// network namespace belongs to a *different* user namespace: the kernel gates `SO_MARK` on
/// `sockopt_ns_capable(sock_net(sk)->user_ns, ...)`, so there the capability can be held and
/// `setsockopt` still fail.
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
