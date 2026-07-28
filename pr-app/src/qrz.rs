//! Minimal QRZ.com XML API client for enriching Address Book entries.
//!
//! Deliberately hand-rolled rather than pulling in a full XML parser: QRZ's
//! response is small and flat, so scanning for `<tag>...</tag>` substrings
//! is enough and matches this project's general preference for minimal
//! dependencies over heavier general-purpose ones.
//!
//! Session flow: log in once with username/password to get a session key
//! (valid ~24h server-side), then look up callsigns with that key. Callers
//! should cache the key (see `AppState::qrz_session`) and only re-login when
//! a lookup reports the session has expired.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

pub struct QrzInfo {
    pub name: Option<String>,
    pub location: Option<String>,
    pub grid: Option<String>,
}

/// Looks up `callsign`, logging in first if `session` is empty and
/// transparently retrying once after a fresh login if the cached session
/// turns out to be expired/invalid.
pub fn lookup(username: &str, password: &str, session: &mut Option<String>, callsign: &str) -> Result<QrzInfo, String> {
    let callsign = strip_ssid(callsign);
    if session.is_none() {
        *session = Some(login(username, password)?);
    }
    match fetch(session.as_deref().unwrap(), callsign) {
        Ok(info) => Ok(info),
        Err(e) if e.to_lowercase().contains("session") => {
            *session = Some(login(username, password)?);
            fetch(session.as_deref().unwrap(), callsign)
        }
        Err(e) => Err(e),
    }
}

/// QRZ doesn't know about AX.25 SSIDs (they're not part of a real amateur
/// callsign) — strip a trailing "-<digits>" like the "-9" in "KD3BFP-9"
/// before looking anything up. Non-numeric suffixes (there shouldn't be any
/// for AX.25, but just in case) are left alone.
fn strip_ssid(callsign: &str) -> &str {
    match callsign.rsplit_once('-') {
        Some((base, ssid)) if !ssid.is_empty() && ssid.chars().all(|c| c.is_ascii_digit()) => base,
        _ => callsign,
    }
}

fn login(username: &str, password: &str) -> Result<String, String> {
    let url = format!(
        "https://xmldata.qrz.com/xml/current/?username={}&password={}",
        utf8_percent_encode(username, NON_ALPHANUMERIC),
        utf8_percent_encode(password, NON_ALPHANUMERIC),
    );
    let body = get(&url)?;
    if let Some(err) = extract_tag(&body, "Error") {
        return Err(err);
    }
    extract_tag(&body, "Key").ok_or_else(|| "QRZ login response had no session key".to_string())
}

fn fetch(session: &str, callsign: &str) -> Result<QrzInfo, String> {
    let url = format!(
        "https://xmldata.qrz.com/xml/current/?s={}&callsign={}",
        utf8_percent_encode(session, NON_ALPHANUMERIC),
        utf8_percent_encode(callsign, NON_ALPHANUMERIC),
    );
    let body = get(&url)?;
    if let Some(err) = extract_tag(&body, "Error") {
        return Err(err);
    }

    let name = match (extract_tag(&body, "fname"), extract_tag(&body, "name")) {
        (Some(f), Some(l)) => Some(format!("{f} {l}")),
        (Some(f), None) => Some(f),
        (None, Some(l)) => Some(l),
        (None, None) => None,
    };
    let location_parts: Vec<String> =
        [extract_tag(&body, "addr2"), extract_tag(&body, "state"), extract_tag(&body, "country")]
            .into_iter()
            .flatten()
            .collect();
    let location = if location_parts.is_empty() { None } else { Some(location_parts.join(", ")) };
    let grid = extract_tag(&body, "grid");

    Ok(QrzInfo { name, location, grid })
}

fn get(url: &str) -> Result<String, String> {
    let mut response = ureq::get(url).call().map_err(|e| e.to_string())?;
    response.body_mut().read_to_string().map_err(|e| e.to_string())
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(unescape_xml(&xml[start..end]))
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_session_key() {
        let xml = "<QRZDatabase><Session><Key>abc123</Key><Count>5</Count></Session></QRZDatabase>";
        assert_eq!(extract_tag(xml, "Key"), Some("abc123".to_string()));
    }

    #[test]
    fn extracts_error() {
        let xml = "<QRZDatabase><Session><Error>Session Timeout</Error></Session></QRZDatabase>";
        assert_eq!(extract_tag(xml, "Error"), Some("Session Timeout".to_string()));
    }

    #[test]
    fn missing_tag_returns_none() {
        let xml = "<QRZDatabase><Session><Key>abc123</Key></Session></QRZDatabase>";
        assert_eq!(extract_tag(xml, "Error"), None);
    }

    #[test]
    fn strip_ssid_removes_numeric_suffix() {
        assert_eq!(strip_ssid("KD3BFP-9"), "KD3BFP");
        assert_eq!(strip_ssid("KD3BFP-15"), "KD3BFP");
    }

    #[test]
    fn strip_ssid_leaves_bare_callsign_alone() {
        assert_eq!(strip_ssid("KD3BFP"), "KD3BFP");
    }

    #[test]
    fn strip_ssid_leaves_non_numeric_suffix_alone() {
        assert_eq!(strip_ssid("KD3BFP-P"), "KD3BFP-P");
    }

    #[test]
    fn unescapes_entities() {
        assert_eq!(unescape_xml("Tom &amp; Jerry"), "Tom & Jerry");
    }
}
