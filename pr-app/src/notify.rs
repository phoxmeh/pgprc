//! Desktop notifications for "directed at me" packet traffic: an unsolicited
//! incoming connection, or a monitored/received frame whose destination matches
//! the configured default callsign. Gated by `NotifyPrefs.directed_enabled`
//! and a global `notifications_silenced` flag. Destination Monitor Rule
//! matches are logged in the same Notifications dialog but handled at the call
//! site in `window.rs` (they always notify unless silenced, with no separate
//! toggle).

use gtk::prelude::*;

use pr_core::AppConfig;

use crate::qrz::strip_ssid;

/// Compiled, ready-to-match form of the user's directed-notification prefs.
/// Rebuilt fresh per event — a handful of string comparisons is free at
/// packet-radio traffic rates.
pub struct NotifyMatcher {
    directed_enabled: bool,
    my_call_base: Option<String>,
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
        NotifyMatcher { directed_enabled: config.notify.directed_enabled, my_call_base }
    }

    /// Returns `true` if `to` is directed at the configured callsign and
    /// directed notifications are enabled. SSID-stripped for the comparison
    /// (a frame to KD3BFP-9 matches a callsign configured as KD3BFP).
    pub fn matches_directed(&self, to: &str) -> bool {
        if !self.directed_enabled {
            return false;
        }
        let check = strip_ssid(to).to_uppercase();
        self.my_call_base.as_deref() == Some(check.as_str())
    }
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

/// Play the configured notification sound, if any, via `paplay` (PulseAudio)
/// or `aplay` (ALSA) on a background thread. Silently no-ops if neither is
/// available or the file doesn't exist.
pub fn play_sound(config: &AppConfig) {
    let Some(path) = config.notify.notification_sound.as_deref().filter(|p| !p.is_empty()) else { return };
    let path = path.to_string();
    std::thread::spawn(move || {
        // Try paplay first (PulseAudio/PipeWire), fall back to aplay (ALSA).
        if std::process::Command::new("paplay").arg(&path).status().is_err() {
            let _ = std::process::Command::new("aplay").arg(&path).status();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pr_core::AppConfig;

    fn config_with(default_call: Option<&str>, directed_enabled: bool) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.ui.default_call = default_call.map(str::to_string);
        cfg.notify.directed_enabled = directed_enabled;
        cfg
    }

    #[test]
    fn disabled_never_matches() {
        let cfg = config_with(Some("KD3BFP-9"), false);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(!matcher.matches_directed("KD3BFP-9"));
    }

    #[test]
    fn matches_my_call_ignoring_ssid() {
        let cfg = config_with(Some("KD3BFP-9"), true);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(matcher.matches_directed("KD3BFP"));
        assert!(matcher.matches_directed("KD3BFP-5"));
        assert!(!matcher.matches_directed("N0CALL-1"));
    }

    #[test]
    fn disabled_does_not_match() {
        let cfg = config_with(Some("KD3BFP-9"), false);
        let matcher = NotifyMatcher::build(&cfg);
        assert!(!matcher.matches_directed("KD3BFP-9"));
    }
}
