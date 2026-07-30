//! Incoming keyboard-to-keyboard mode: when enabled, an unsolicited
//! connection addressed to this station's own callsign gets a welcome
//! message and is left as a normal live session tab for a human to type
//! into directly -- unlike the personal mailbox, there's no command parser
//! here at all.

pub fn default_welcome(my_call: &str) -> String {
    format!("{my_call} here, ready for keyboard-to-keyboard. Go ahead.\n")
}

/// Whether an unsolicited incoming connection addressed to `to` should
/// trigger keyboard-to-keyboard mode. `node_call` is `KeyboardModePrefs
/// .node_call` (falling back to `UiPrefs.default_call` if unset -- resolved
/// by the caller, not here) -- an unset/empty value can't be confirmed as
/// "addressed to me", so it's declined rather than assumed. Unlike the
/// mailbox's `respond_call`, there's no "match anything" empty-string
/// convenience here: this is a new feature with no back-compat behavior to
/// preserve, and always requiring an explicit identity is what keeps this
/// mode from colliding with the mailbox's own (independently configurable)
/// callsign.
pub fn should_answer(enabled: bool, node_call: &str, to: Option<&str>) -> bool {
    if !enabled {
        return false;
    }
    let node_call = node_call.trim();
    if node_call.is_empty() {
        return false;
    }
    to.map(|t| t.eq_ignore_ascii_case(node_call)).unwrap_or(false)
}

/// Whether `port_id` is one of the ports a feature (keyboard mode or
/// mailbox) should listen on -- an empty list means "any port", matching
/// each feature's original behavior before per-port filtering existed.
pub fn listens_on(listen_ports: &[String], port_id: &str) -> bool {
    listen_ports.is_empty() || listen_ports.iter().any(|p| p == port_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declines_when_disabled() {
        assert!(!should_answer(false, "KD3BFP-9", Some("KD3BFP-9")));
    }

    #[test]
    fn declines_with_no_configured_callsign() {
        assert!(!should_answer(true, "", Some("KD3BFP-9")));
    }

    #[test]
    fn matches_own_callsign_case_insensitively_and_only_that() {
        assert!(should_answer(true, "kd3bfp-9", Some("KD3BFP-9")));
        assert!(!should_answer(true, "KD3BFP-9", Some("KD3BFP-6")));
        assert!(!should_answer(true, "KD3BFP-9", None));
    }

    #[test]
    fn listens_on_empty_list_means_any_port() {
        assert!(listens_on(&[], "port-1"));
    }

    #[test]
    fn listens_on_respects_explicit_list() {
        let ports = vec!["port-1".to_string(), "port-2".to_string()];
        assert!(listens_on(&ports, "port-1"));
        assert!(!listens_on(&ports, "port-3"));
    }
}
