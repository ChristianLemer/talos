// Port de decision.js + version.js — la règle PURE de Talos, SOURCE UNIQUE (au
// cœur, décision multi-frontend : les clients affichent, ne décident pas).
// Deux questions : quel est l'état DÉSIRÉ (posture + toggle), et quelle ACTION
// un Apply prendrait (désir × réalité machine × pin).

use crate::bundles::Posture;

/// Compare deux versions pointées, NUMÉRIQUEMENT segment par segment (2.10 > 2.9).
/// Segment = préfixe numérique (parseInt : "0-beta" → 0). Segment manquant = 0.
/// Retourne -1 (a<b) / 0 (==) / 1 (a>b). Pas full semver (voir talos-version-pin).
pub fn compare_versions(a: &str, b: &str) -> i32 {
    let seg = |v: &str| -> Vec<i64> {
        v.split('.')
            .map(|s| {
                let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<i64>().unwrap_or(0)
            })
            .collect()
    };
    let (as_, bs) = (seg(a), seg(b));
    let n = as_.len().max(bs.len());
    for i in 0..n {
        let x = as_.get(i).copied().unwrap_or(0);
        let y = bs.get(i).copied().unwrap_or(0);
        if x < y {
            return -1;
        }
        if x > y {
            return 1;
        }
    }
    0
}

// Ces 4 fonctions (posture → désir) sont la moitié "résolution d'intention" de la
// règle. Aujourd'hui le FRONT la résout et envoie on/off au serveur ; elles sont
// donc encore non appelées côté Rust, mais testées et prêtes pour quand le serveur
// portera la résolution complète (chemin B / une seule source). Gardées exprès.
#[allow(dead_code)]
/// Où le toggle démarre (défaut auteur) : mandatory/opt-out → in ; opt-in/forbidden → out.
pub fn posture_default_in(posture: &Posture) -> bool {
    matches!(posture, Posture::Mandatory | Posture::OptOut)
}

#[allow(dead_code)]
/// L'utilisateur peut-il bouger le toggle ? mandatory/forbidden = verrouillé.
pub fn is_locked(posture: &Posture) -> bool {
    matches!(posture, Posture::Mandatory | Posture::Forbidden)
}

/// L'état effectif in/out. Verrouillé → défaut auteur (l'auteur gagne). Sinon le
/// toggle utilisateur (Some(true)=in, Some(false)=out), ou le défaut si None.
fn toggle_in(posture: &Posture, user_toggle: Option<bool>) -> bool {
    if is_locked(posture) {
        return posture_default_in(posture);
    }
    user_toggle.unwrap_or_else(|| posture_default_in(posture))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Desired {
    Present,
    Absent,
}

#[allow(dead_code)]
/// L'état DÉSIRÉ (present|absent) depuis posture + toggle utilisateur.
pub fn desired_state(posture: &Posture, user_toggle: Option<bool>) -> Desired {
    if toggle_in(posture, user_toggle) {
        Desired::Present
    } else {
        Desired::Absent
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Install,
    Uninstall,
    Upgrade,
    Downgrade,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Uninstall => "uninstall",
            Action::Upgrade => "upgrade",
            Action::Downgrade => "downgrade",
        }
    }
}

pub struct MachineFacts<'a> {
    pub present: bool,
    pub outdated: bool,
    pub can_uninstall: bool,
    pub pin: Option<&'a str>,
    pub installed_version: &'a str,
}

/// L'action qu'un Apply prendrait — ou None. La règle que le serveur EXÉCUTE et
/// que le front prévisualise (source unique). Un pin recadre "outdated" : la
/// référence devient le pin, pas "latest".
///   present désiré & absent            → install
///   present désiré & présent & <pin    → upgrade (vers le pin)
///   present désiré & présent & >pin    → downgrade (MANUEL, filtré hors Apply)
///   present désiré & présent & périmé (sans pin) → upgrade (latest)
///   absent désiré  & présent & removable → uninstall
pub fn action_for(desired: Desired, f: &MachineFacts) -> Option<Action> {
    if desired == Desired::Present && !f.present {
        return Some(Action::Install);
    }
    if desired == Desired::Present && f.present {
        if let Some(pin) = f.pin.filter(|p| !p.is_empty()) {
            return match compare_versions(f.installed_version, pin) {
                c if c < 0 => Some(Action::Upgrade),
                c if c > 0 => Some(Action::Downgrade),
                _ => None, // au pin → satisfait
            };
        }
        if f.outdated {
            return Some(Action::Upgrade);
        }
    }
    if desired == Desired::Absent && f.present && f.can_uninstall {
        return Some(Action::Uninstall);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_numeric_not_string() {
        assert_eq!(compare_versions("2.10", "2.9"), 1); // numérique, pas string
        assert_eq!(compare_versions("1.8", "1.8.0"), 0); // segment manquant = 0
        assert_eq!(compare_versions("1.0", "1.0.1"), -1);
        assert_eq!(compare_versions("0-beta", "0"), 0); // suffixe pré-release ignoré
    }

    #[test]
    fn posture_defaults() {
        assert!(posture_default_in(&Posture::Mandatory));
        assert!(posture_default_in(&Posture::OptOut));
        assert!(!posture_default_in(&Posture::OptIn));
        assert!(!posture_default_in(&Posture::Forbidden));
    }

    #[test]
    fn desired_opt_in_untouched_is_absent() {
        assert_eq!(desired_state(&Posture::OptIn, None), Desired::Absent);
        assert_eq!(desired_state(&Posture::OptIn, Some(true)), Desired::Present);
    }

    #[test]
    fn locked_ignores_toggle() {
        // forbidden verrouillé → toujours absent même si l'utilisateur coche "in"
        assert_eq!(desired_state(&Posture::Forbidden, Some(true)), Desired::Absent);
    }

    #[test]
    fn action_install_when_absent() {
        let f = MachineFacts { present: false, outdated: false, can_uninstall: false, pin: None, installed_version: "" };
        assert_eq!(action_for(Desired::Present, &f), Some(Action::Install));
    }

    #[test]
    fn action_upgrade_when_stale() {
        let f = MachineFacts { present: true, outdated: true, can_uninstall: false, pin: None, installed_version: "1.0" };
        assert_eq!(action_for(Desired::Present, &f), Some(Action::Upgrade));
    }

    #[test]
    fn action_pin_below_upgrades_above_downgrades() {
        let below = MachineFacts { present: true, outdated: false, can_uninstall: false, pin: Some("2.0"), installed_version: "1.0" };
        assert_eq!(action_for(Desired::Present, &below), Some(Action::Upgrade));
        let above = MachineFacts { present: true, outdated: false, can_uninstall: false, pin: Some("2.0"), installed_version: "3.0" };
        assert_eq!(action_for(Desired::Present, &above), Some(Action::Downgrade));
        let at = MachineFacts { present: true, outdated: false, can_uninstall: false, pin: Some("2.0"), installed_version: "2.0" };
        assert_eq!(action_for(Desired::Present, &at), None);
    }

    #[test]
    fn action_uninstall_only_when_removable() {
        let removable = MachineFacts { present: true, outdated: false, can_uninstall: true, pin: None, installed_version: "" };
        assert_eq!(action_for(Desired::Absent, &removable), Some(Action::Uninstall));
        let not = MachineFacts { present: true, outdated: false, can_uninstall: false, pin: None, installed_version: "" };
        assert_eq!(action_for(Desired::Absent, &not), None);
    }
}
