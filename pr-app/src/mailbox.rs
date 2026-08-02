//! A minimal BBS-style personal packet mailbox: other stations connect and
//! interact with a short command prompt to read back messages they've left,
//! or log a CQ contact with their QTH. Local store-and-forward only —
//! not compatible with real Winlink/RMS network infrastructure.

use pr_core::{MailboxMessage, QsoLogEntry};

/// Where a mailbox-driven connection currently is in the command grammar.
pub enum MailboxState {
    Command,
    /// Waiting for the remote station's QTH after they typed `CQ`.
    AwaitingQth,
}

pub fn welcome_banner(my_call: &str) -> String {
    format!("{my_call} BBS. Commands: L)ist  R)ead <id>  CQ  B)ye\n")
}

/// Whether an unsolicited incoming connection addressed to `to` should be
/// auto-answered by the mailbox, given its enabled flag and the configured
/// respond-from callsign. Case-insensitive exact match against the
/// destination the connect request actually carried. A blank
/// `respond_call` always declines -- the mailbox requires an explicit
/// callsign of its own (never falling back to the general Profile
/// callsign) and can't even be enabled without one (enforced in
/// `mailbox_dialog`'s Enable button), so this is really just a defensive
/// second check, not the primary guard.
pub fn should_answer(enabled: bool, respond_call: &str, to: Option<&str>) -> bool {
    if !enabled {
        return false;
    }
    let respond_call = respond_call.trim();
    if respond_call.is_empty() {
        return false;
    }
    to.map(|t| t.eq_ignore_ascii_case(respond_call)).unwrap_or(false)
}

/// Which shared color-state CSS class (if any) the mailbox's controls
/// should show: an unread message is more urgent/attention-grabbing than
/// merely "enabled", so it takes priority over the plain enabled/green
/// state.
pub fn status_class(enabled: bool, has_unread: bool) -> Option<&'static str> {
    if has_unread {
        Some("state-warning")
    } else if enabled {
        Some("state-success")
    } else {
        None
    }
}

fn prompt() -> String {
    "Cmd: ".to_string()
}

/// Advance the mailbox state machine by one line of input from the
/// connected station. Returns `(reply_text, close_connection, new_qso_entry)`.
/// The caller is responsible for appending `new_qso_entry` to the QSO log
/// when `Some`; keeping it separate avoids a double-mutable-borrow on the
/// `AppConfig` at the call site. `timestamp` and `port_id` are supplied by
/// the caller so this module stays free of GTK dependencies and remains
/// independently unit-testable.
pub fn handle_line(
    state: &mut MailboxState,
    messages: &mut Vec<MailboxMessage>,
    remote_call: &str,
    port_id: &str,
    line: &str,
    timestamp: &str,
) -> (String, bool, Option<QsoLogEntry>) {
    let line = line.trim();
    match state {
        MailboxState::Command => {
            let (text, close) = handle_command(messages, remote_call, line, state);
            (text, close, None)
        }
        MailboxState::AwaitingQth => {
            let location = if line.is_empty() { None } else { Some(line.to_string()) };
            let entry = QsoLogEntry {
                callsign: remote_call.to_string(),
                port_id: port_id.to_string(),
                started: timestamp.to_string(),
                ended: None,
                location: location.clone(),
                from_cq: true,
            };
            *state = MailboxState::Command;
            let qth_line = match &location {
                Some(qth) => format!("QTH logged: {qth}\n"),
                None => String::new(),
            };
            (format!("{qth_line}Contact logged. 73 de {remote_call}!\n{}", prompt()), false, Some(entry))
        }
    }
}

fn handle_command(messages: &mut [MailboxMessage], remote_call: &str, line: &str, state: &mut MailboxState) -> (String, bool) {
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("").to_uppercase();
    let arg = parts.next().unwrap_or("").trim();

    match cmd.as_str() {
        "L" | "LIST" => {
            // Show messages this station previously left here.
            let sent: Vec<&MailboxMessage> = messages.iter().filter(|m| m.from.eq_ignore_ascii_case(remote_call)).collect();
            if sent.is_empty() {
                (format!("No messages on file from you.\n{}", prompt()), false)
            } else {
                let mut out = String::new();
                for m in sent {
                    let read_flag = if m.read { "R" } else { " " };
                    out.push_str(&format!("{:>4}  {}  [{read_flag}]  to {}  {}\n", m.id, m.timestamp, m.to, m.subject));
                }
                out.push_str(&prompt());
                (out, false)
            }
        }
        "R" | "READ" => {
            let Some(id) = arg.parse::<u64>().ok() else {
                return (format!("Usage: R <id>\n{}", prompt()), false);
            };
            // Only allow reading back messages this station sent.
            match messages.iter().find(|m| m.id == id && m.from.eq_ignore_ascii_case(remote_call)) {
                Some(m) => {
                    (format!("To: {}\nSubject: {}\n\n{}\n{}", m.to, m.subject, m.body, prompt()), false)
                }
                None => (format!("No such message.\n{}", prompt()), false),
            }
        }
        "CQ" => {
            *state = MailboxState::AwaitingQth;
            ("QTH (city, state/country or grid locator): ".to_string(), false)
        }
        "B" | "BYE" => ("73, disconnecting...\n".to_string(), true),
        _ => (format!("Unknown command.\n{}", prompt()), false),
    }
}

fn _next_id(messages: &[MailboxMessage]) -> u64 {
    messages.iter().map(|m| m.id).max().unwrap_or(0) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = "2026-07-27 20:45:39";
    const PORT: &str = "port-0";

    fn run(state: &mut MailboxState, messages: &mut Vec<MailboxMessage>, from: &str, line: &str) -> (String, bool, Option<QsoLogEntry>) {
        handle_line(state, messages, from, PORT, line, TS)
    }

    #[test]
    fn list_shows_messages_sent_by_connecting_station() {
        let mut messages = vec![
            MailboxMessage { id: 1, to: "KD3BFP-5".to_string(), from: "N0CALL-1".to_string(), subject: "Hello".to_string(), body: "hi".to_string(), timestamp: TS.to_string(), read: false },
            MailboxMessage { id: 2, to: "OTHER".to_string(), from: "THIRD-1".to_string(), subject: "Other".to_string(), body: "x".to_string(), timestamp: TS.to_string(), read: false },
        ];
        let mut state = MailboxState::Command;
        let (resp, _, _) = run(&mut state, &mut messages, "N0CALL-1", "L");
        assert!(resp.contains("Hello"));
        assert!(!resp.contains("Other"));
    }

    #[test]
    fn read_only_allows_own_messages() {
        let mut messages = vec![
            MailboxMessage { id: 1, to: "KD3BFP-5".to_string(), from: "N0CALL-1".to_string(), subject: "Test".to_string(), body: "body".to_string(), timestamp: TS.to_string(), read: false },
        ];
        let mut state = MailboxState::Command;
        let (resp, _, _) = run(&mut state, &mut messages, "N0CALL-1", "R 1");
        assert!(resp.contains("body"));
        let mut state2 = MailboxState::Command;
        let (resp2, _, _) = run(&mut state2, &mut messages, "OTHER-1", "R 1");
        assert!(resp2.contains("No such message"));
    }

    #[test]
    fn cq_logs_contact_with_qth() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;
        let (prompt_resp, close, entry) = run(&mut state, &mut messages, "N0CALL-1", "CQ");
        assert!(!close);
        assert!(entry.is_none());
        assert!(prompt_resp.to_lowercase().contains("qth"));

        let (resp, close, entry) = run(&mut state, &mut messages, "N0CALL-1", "Pittsburgh, PA");
        assert!(!close);
        assert!(resp.contains("Pittsburgh"));
        assert!(resp.contains("logged"));
        let entry = entry.expect("CQ should produce a QsoLogEntry");
        assert_eq!(entry.location.as_deref(), Some("Pittsburgh, PA"));
        assert_eq!(entry.callsign, "N0CALL-1");
    }

    #[test]
    fn cq_accepts_blank_qth() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;
        run(&mut state, &mut messages, "N0CALL-1", "CQ");
        let (resp, _, entry) = run(&mut state, &mut messages, "N0CALL-1", "");
        assert!(resp.contains("logged"));
        assert_eq!(entry.expect("should produce entry").location, None);
    }

    #[test]
    fn bye_signals_close() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;
        let (_, close, _) = run(&mut state, &mut messages, "N0CALL-1", "B");
        assert!(close);
    }

    #[test]
    fn unknown_command_reprompts_without_closing() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;
        let (resp, close, _) = run(&mut state, &mut messages, "N0CALL-1", "ZZZ");
        assert!(!close);
        assert!(resp.contains("Unknown command"));
    }

    #[test]
    fn should_answer_respects_enabled_flag() {
        assert!(!should_answer(false, "", Some("KD3BFP-5")));
        assert!(!should_answer(false, "KD3BFP-5", Some("KD3BFP-5")));
    }

    #[test]
    fn should_answer_declines_with_no_respond_call() {
        assert!(!should_answer(true, "", Some("KD3BFP-1")));
        assert!(!should_answer(true, "", None));
    }

    #[test]
    fn should_answer_only_matches_configured_callsign() {
        assert!(should_answer(true, "KD3BFP-5", Some("KD3BFP-5")));
        assert!(!should_answer(true, "KD3BFP-5", Some("KD3BFP-6")));
        assert!(should_answer(true, " kd3bfp-5 ", Some("KD3BFP-5")));
        assert!(!should_answer(true, "KD3BFP-5", None));
    }

    #[test]
    fn status_class_prioritizes_unread_over_enabled() {
        assert_eq!(status_class(false, false), None);
        assert_eq!(status_class(true, false), Some("state-success"));
        assert_eq!(status_class(false, true), Some("state-warning"));
        assert_eq!(status_class(true, true), Some("state-warning"));
    }
}
