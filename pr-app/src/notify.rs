//! Desktop notifications for "directed at me" packet traffic: an unsolicited
//! incoming connection, a monitored/received frame whose destination matches
//! the configured default callsign, or a user's `HighlightRule` with its
//! `notify` flag set (the same destination-address rules used for
//! highlighting also drive notifications — one rule list, two effects).
//! Gated by two independent `NotifyPrefs` toggles (`directed_enabled`/
//! `custom_enabled`) since they're genuinely different things to want on or
//! off separately — both off by default, same precedent as the personal
//! mailbox. Beacon-detection notifications are a separate concern entirely
//! (see `window.rs::maybe_detect_beacon`), not handled by this matcher.

use gtk::prelude::*;
use regex::{Regex, RegexBuilder};

use pr_core::{AppConfig, HighlightRule};

use crate::qrz::strip_ssid;

/// Which kind of destination match fired, so the caller can pick the right
/// notification title/wording without this module needing to know about UI
/// formatting.
pub enum NotifyMatch {
    Directed,
    Custom(String),
}

/// Compiled, ready-to-match form of the user's notification preferences.
/// Rebuilt fresh per event rather than cached, mirroring `Highlighter` — a
/// handful of small regexes is free at packet-radio traffic rates.
pub struct NotifyMatcher {
    directed_enabled: bool,
    custom_enabled: bool,
    my_call_base: Option<String>,
    rules: Vec<(Regex, String)>,
}

impl NotifyMatcher {
    pub fn build(config: &AppConfig) -> Self {
        let my_call_base = config
            .ui
            .default_call
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| strip_ssid(s).to_uppercase());
        let rules =
            config.highlighting.rules.iter().filter(|r| r.enabled && r.notify).filter_map(build_rule).collect();
        NotifyMatcher {
            directed_enabled: config.notify.directed_enabled,
            custom_enabled: config.notify.custom_enabled,
            my_call_base,
            rules,
        }
    }

    /// If `to` should trigger a notification, returns which kind matched;
    /// `None` otherwise (both toggles off, or no match). Directed takes
    /// precedence over a custom-rule match when both would apply.
    pub fn match_destination(&self, to: &str) -> Option<NotifyMatch> {
        // SSID-stripped only for the "is this my callsign" check — a
        // custom rule's pattern is matched verbatim (uppercased) since
        // digipeater aliases like `WIDE1-1`/`WIDE2-2` use a trailing `-N`
        // as a hop count, not an SSID, and stripping it would silently
        // break those patterns.
        let my_call_check = strip_ssid(to).to_uppercase();
        if self.directed_enabled && self.my_call_base.as_deref() == Some(my_call_check.as_str()) {
            return Some(NotifyMatch::Directed);
        }
        if self.custom_enabled {
            let upper = to.trim().to_uppercase();
            if let Some((_, label)) = self.rules.iter().find(|(re, _)| re.is_match(&upper)) {
                return Some(NotifyMatch::Custom(label.clone()));
            }
        }
        None
    }
}

fn build_rule(rule: &HighlightRule) -> Option<(Regex, String)> {
    let alts: Vec<String> = rule.pattern.split([',', '|']).map(str::trim).filter(|s| !s.is_empty()).map(regex::escape).collect();
    if alts.is_empty() {
        return None;
    }
    // Exact match against the base callsign — a destination *is* the
    // whole address field, not a substring to search within.
    let pattern = format!("^({})$", alts.join("|"));
    RegexBuilder::new(&pattern).case_insensitive(true).build().ok().map(|re| (re, rule.label.clone()))
}

/// Send a desktop notification through the app's `gio::Application`, if one
/// is reachable from `window` (always true once the window is presented).
pub fn send(window: &impl IsA<gtk::Window>, title: &str, body: &str) {
    let Some(app) = window.upcast_ref::<gtk::Window>().application() else { return };
    let notification = gtk::gio::Notification::new(title);
    notification.set_body(Some(body));
    notification.set_priority(gtk::gio::NotificationPriority::Normal);
    app.send_notification(None, &notification);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pr_core::AppConfig;

    fn config_with(default_call: Option<&str>, directed_enabled: bool, custom_enabled: bool, rules: Vec<HighlightRule>) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.ui.default_call = default_call.map(str::to_string);
        cfg.notify.directed_enabled = directed_enabled;
        cfg.notify.custom_enabled = custom_enabled;
        cfg.highlighting.rules = rules;
        cfg
    }

    fn rule(label: &str, pattern: &str, notify: bool, enabled: bool) -> HighlightRule {
        HighlightRule { label: label.to_string(), pattern: pattern.to_string(), color: "#FFFFFF".to_string(), notify, enabled }
    }

    #[test]
    fn disabled_never_matches() {
        let cfg = config_with(Some("KD3BFP-9"), false, false, vec![]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.match_destination("KD3BFP-9").is_none());
    }

    #[test]
    fn matches_my_call_ignoring_ssid() {
        let cfg = config_with(Some("KD3BFP-9"), true, false, vec![]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matches!(matcher.match_destination("KD3BFP"), Some(NotifyMatch::Directed)));
        assert!(matches!(matcher.match_destination("KD3BFP-5"), Some(NotifyMatch::Directed)));
        assert!(matcher.match_destination("N0CALL-1").is_none());
    }

    #[test]
    fn directed_disabled_does_not_match_even_with_custom_enabled() {
        let cfg = config_with(Some("KD3BFP-9"), false, true, vec![]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.match_destination("KD3BFP-9").is_none());
    }

    #[test]
    fn matches_custom_destination_rule() {
        let cfg = config_with(None, false, true, vec![rule("Wide digi", "WIDE1-1, WIDE2-1", true, true)]);
        let matcher = NotifyMatcher::build(&cfg);
        match matcher.match_destination("wide1-1") {
            Some(NotifyMatch::Custom(label)) => assert_eq!(label, "Wide digi"),
            other => panic!("expected a custom-rule match, got {}", other.is_some()),
        }
        assert!(matcher.match_destination("WIDE3-3").is_none());
    }

    #[test]
    fn custom_disabled_does_not_match_even_with_directed_enabled() {
        let cfg = config_with(None, true, false, vec![rule("CQ", "CQ", true, true)]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.match_destination("CQ").is_none());
    }

    #[test]
    fn rule_without_notify_flag_is_ignored() {
        // notify: false — a highlight-only rule shouldn't also fire
        // notifications.
        let cfg = config_with(None, false, true, vec![rule("CQ", "CQ", false, true)]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.match_destination("CQ").is_none());
    }

    #[test]
    fn disabled_rule_is_ignored() {
        let cfg = config_with(None, false, true, vec![rule("Off", "CQ", true, false)]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.match_destination("CQ").is_none());
    }

    #[test]
    fn rule_pattern_is_exact_not_substring() {
        // "CQ" shouldn't match "CQD" or vice versa — a destination field is
        // the whole address, not text to search within.
        let cfg = config_with(None, false, true, vec![rule("CQ", "CQ", true, true)]);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.match_destination("CQD").is_none());
    }
}
