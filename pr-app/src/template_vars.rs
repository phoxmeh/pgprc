//! `$$VAR` placeholder substitution for automated message text (beacons,
//! mailbox/keyboard-to-keyboard greetings). Only the specific recognized
//! all-caps names below are ever touched -- anything else shaped like
//! `$$WORD` is left as literal text, not treated as an error or a typo to
//! guess at.

/// The values every placeholder resolves from. `node` is resolved by the
/// caller (the mailbox's own callsign, keyboard-to-keyboard's own node
/// callsign, or the general Profile callsign for anything else), since
/// "which callsign is this" is genuinely different per feature -- the rest
/// (`name`/`loc`/`bbs_home`) always come from the general Profile settings
/// regardless of which feature is substituting.
pub struct TemplateVars {
    pub node: String,
    pub name: String,
    pub loc: String,
    pub bbs_home: String,
}

impl TemplateVars {
    pub fn from_config(cfg: &pr_core::AppConfig, node: impl Into<String>) -> Self {
        TemplateVars {
            node: node.into(),
            name: cfg.ui.name.clone().unwrap_or_default(),
            loc: cfg.ui.location.clone().unwrap_or_default(),
            bbs_home: cfg.ui.home_bbs.clone().unwrap_or_default(),
        }
    }

    /// Replace every recognized `$$WORD` placeholder in `text`. A missing/
    /// empty value is replaced with nothing at all, per explicit request --
    /// never left as the literal placeholder or some other filler text.
    pub fn apply(&self, text: &str) -> String {
        text.replace("$$NODE", &self.node).replace("$$NAME", &self.name).replace("$$LOC", &self.loc).replace("$$BBSHOME", &self.bbs_home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(node: &str, name: &str, loc: &str, bbs_home: &str) -> TemplateVars {
        TemplateVars { node: node.to_string(), name: name.to_string(), loc: loc.to_string(), bbs_home: bbs_home.to_string() }
    }

    #[test]
    fn substitutes_all_known_variables() {
        let v = vars("KD3BFP-9", "Dave", "Grid FM19", "KD3BFP-9.#EPA.PA.USA.NOAM");
        assert_eq!(
            v.apply("Node: $$NODE, Op: $$NAME, Loc: $$LOC, BBS: $$BBSHOME"),
            "Node: KD3BFP-9, Op: Dave, Loc: Grid FM19, BBS: KD3BFP-9.#EPA.PA.USA.NOAM"
        );
    }

    #[test]
    fn empty_values_become_empty_string_not_a_blank_placeholder() {
        let v = vars("KD3BFP-9", "", "", "");
        assert_eq!(v.apply("Hi, I'm $$NODE ($$NAME)"), "Hi, I'm KD3BFP-9 ()");
    }

    #[test]
    fn unrecognized_placeholder_is_left_untouched() {
        let v = vars("KD3BFP-9", "Dave", "", "");
        assert_eq!(v.apply("$$UNKNOWN stays as-is, $$NODE doesn't"), "$$UNKNOWN stays as-is, KD3BFP-9 doesn't");
    }

    #[test]
    fn text_without_placeholders_is_unchanged() {
        let v = vars("X", "Y", "Z", "W");
        assert_eq!(v.apply("no placeholders here"), "no placeholders here");
    }

    #[test]
    fn repeated_placeholder_is_substituted_every_time() {
        let v = vars("KD3BFP-9", "", "", "");
        assert_eq!(v.apply("$$NODE de $$NODE"), "KD3BFP-9 de KD3BFP-9");
    }

    #[test]
    fn from_config_maps_profile_fields_and_uses_caller_supplied_node() {
        let mut cfg = pr_core::AppConfig::default();
        cfg.ui.name = Some("Dave".to_string());
        cfg.ui.location = Some("Grid FM19".to_string());
        cfg.ui.home_bbs = Some("KD3BFP-9.#EPA.PA.USA.NOAM".to_string());
        // `default_call` deliberately not read by `from_config` -- the
        // caller resolves `node` itself (mailbox callsign, keyboard-to-
        // keyboard identity, or Profile callsign for anything else).
        cfg.ui.default_call = Some("SHOULD-NOT-APPEAR".to_string());

        let vars = TemplateVars::from_config(&cfg, "KD3BFP-1");
        assert_eq!(vars.apply("$$NODE/$$NAME/$$LOC/$$BBSHOME"), "KD3BFP-1/Dave/Grid FM19/KD3BFP-9.#EPA.PA.USA.NOAM");
    }

    #[test]
    fn from_config_unset_profile_fields_become_empty() {
        let cfg = pr_core::AppConfig::default();
        let vars = TemplateVars::from_config(&cfg, "KD3BFP-1");
        assert_eq!(vars.apply("[$$NODE][$$NAME][$$LOC][$$BBSHOME]"), "[KD3BFP-1][][][]");
    }
}
