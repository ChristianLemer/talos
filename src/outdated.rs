// Port de outdated.ts — un scan machine-wide via le manager natif. Best-effort :
// pas de manager / tout échec → map vide (direction sûre "rien de périmé").
use std::collections::HashMap;

use crate::managers::{native_manager, Outdated};
use crate::platform::{shell_probe, Os};

pub fn scan_outdated(os: Os) -> HashMap<String, Outdated> {
    let Some(mgr) = native_manager(os) else {
        return HashMap::new();
    };
    let probe = shell_probe(os, &mgr.outdated_scan_command());
    match crate::platform::quiet_command(&probe.cmd)
        .args(&probe.args)
        .output()
    {
        Ok(o) => mgr.parse_outdated(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => HashMap::new(),
    }
}

/// Le systemId (lowercased) d'un paquet → son entrée outdated, ou None.
pub fn outdated_for<'a>(
    system_id: Option<&str>,
    scan: &'a HashMap<String, Outdated>,
) -> Option<&'a Outdated> {
    scan.get(&system_id?.to_lowercase())
}
