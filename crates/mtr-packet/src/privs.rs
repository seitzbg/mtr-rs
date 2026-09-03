//! Drop setuid privileges and every capability once the sockets are open. Ported from
//! packet/packet.c:43-102 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use caps::CapSet;
use nix::unistd::{getegid, geteuid, getgid, getuid, setgid, setuid};

use crate::Fatal;

/// `drop_elevated_permissions()`: `setgid(getgid())`, `setuid(getuid())`, verify, then clear
/// the effective, permitted and inheritable sets (`cap_clear` + `cap_set_proc`) so nothing
/// after this point can regain privilege.
pub fn drop_all() -> Result<(), Fatal> {
    let perm = |e: nix::Error| Fatal::Message(format!("Unable to drop elevated permissions: {e}"));
    setgid(getgid()).map_err(perm)?;
    setuid(getuid()).map_err(perm)?;
    if geteuid() != getuid() || getegid() != getgid() {
        return Err(Fatal::Message("Unable to drop elevated permissions".into()));
    }
    for set in [CapSet::Effective, CapSet::Inheritable, CapSet::Permitted] {
        caps::clear(None, set)
            .map_err(|e| Fatal::Message(format!("Failed to drop capabilities: {e}")))?;
    }
    Ok(())
}
