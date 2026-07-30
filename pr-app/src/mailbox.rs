//! A minimal BBS-style personal packet mailbox: other stations connect and
//! interact with a short command prompt to leave/read text messages. Local
//! store-and-forward only — not compatible with real Winlink/RMS network
//! infrastructure.

use pr_core::MailboxMessage;

/// Where a mailbox-driven connection currently is in the command grammar.
pub enum MailboxState {
    Command,
    SendSubject { to: String },
    SendBody { to: String, subject: String, body: Vec<String> },
}

pub fn welcome_banner(my_call: &str) -> String {
    format!("{my_call} BBS. Commands: L)ist  R)ead <id>  S)end <call>  B)ye\n")
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
/// connected station. Returns the text to send back, and whether the
/// connection should now be closed. Mutates `messages` in place when a
/// `Send` completes. `timestamp` is supplied by the caller (rather than
/// computed here) so this module stays free of a GTK dependency and
/// independently unit-testable, while still using the same human-readable
/// format the rest of the app persists (see `AppState`'s `now_timestamp`).
pub fn handle_line(
    state: &mut MailboxState,
    messages: &mut Vec<MailboxMessage>,
    remote_call: &str,
    line: &str,
    timestamp: &str,
) -> (String, bool) {
    let line = line.trim();
    match state {
        MailboxState::Command => handle_command(messages, remote_call, line, state),
        MailboxState::SendSubject { to } => {
            let to = to.clone();
            if line.is_empty() {
                return ("Subject is required.\n".to_string(), false);
            }
            let subject = line.to_string();
            *state = MailboxState::SendBody { to, subject, body: Vec::new() };
            ("Body (end with '.' alone on a line):\n".to_string(), false)
        }
        MailboxState::SendBody { to, subject, body } => {
            if line == "." {
                let to = to.clone();
                let msg = MailboxMessage {
                    id: next_id(messages),
                    to: to.clone(),
                    from: remote_call.to_string(),
                    subject: subject.clone(),
                    body: body.join("\n"),
                    timestamp: timestamp.to_string(),
                    read: false,
                };
                messages.push(msg);
                *state = MailboxState::Command;
                (format!("Message saved for {}.\n{}", to, prompt()), false)
            } else {
                body.push(line.to_string());
                (String::new(), false)
            }
        }
    }
}

fn handle_command(messages: &mut [MailboxMessage], remote_call: &str, line: &str, state: &mut MailboxState) -> (String, bool) {
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("").to_uppercase();
    let arg = parts.next().unwrap_or("").trim();

    match cmd.as_str() {
        "L" | "LIST" => {
            let unread: Vec<&MailboxMessage> = messages.iter().filter(|m| m.to == remote_call && !m.read).collect();
            if unread.is_empty() {
                (format!("No messages.\n{}", prompt()), false)
            } else {
                let mut out = String::new();
                for m in unread {
                    out.push_str(&format!("{:>4}  {}  from {}  {}\n", m.id, m.timestamp, m.from, m.subject));
                }
                out.push_str(&prompt());
                (out, false)
            }
        }
        "R" | "READ" => {
            let Some(id) = arg.parse::<u64>().ok() else {
                return (format!("Usage: R <id>\n{}", prompt()), false);
            };
            match messages.iter_mut().find(|m| m.id == id && m.to == remote_call) {
                Some(m) => {
                    m.read = true;
                    (format!("From: {}\nSubject: {}\n\n{}\n{}", m.from, m.subject, m.body, prompt()), false)
                }
                None => (format!("No such message.\n{}", prompt()), false),
            }
        }
        "S" | "SEND" => {
            if arg.is_empty() {
                return (format!("Usage: S <callsign>\n{}", prompt()), false);
            }
            let to = arg.to_uppercase();
            *state = MailboxState::SendSubject { to };
            ("Subject: ".to_string(), false)
        }
        "B" | "BYE" => ("73, disconnecting...\n".to_string(), true),
        _ => (format!("Unknown command.\n{}", prompt()), false),
    }
}

fn next_id(messages: &[MailboxMessage]) -> u64 {
    messages.iter().map(|m| m.id).max().unwrap_or(0) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = "2026-07-27 20:45:39";

    #[test]
    fn full_send_then_list_then_read_flow() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;

        let (resp, close) = handle_line(&mut state, &mut messages, "N0CALL-1", "S KD3BFP-9", TS);
        assert!(!close);
        assert!(resp.contains("Subject"));

        let (_, close) = handle_line(&mut state, &mut messages, "N0CALL-1", "Test Subject", TS);
        assert!(!close);

        let (_, close) = handle_line(&mut state, &mut messages, "N0CALL-1", "line one", TS);
        assert!(!close);
        let (resp, close) = handle_line(&mut state, &mut messages, "N0CALL-1", ".", TS);
        assert!(!close);
        assert!(resp.contains("saved"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].to, "KD3BFP-9");
        assert_eq!(messages[0].from, "N0CALL-1");
        assert_eq!(messages[0].body, "line one");
        assert_eq!(messages[0].timestamp, TS);

        // A different station listing sees nothing addressed to them.
        let (resp, _) = handle_line(&mut state, &mut messages, "N0CALL-1", "L", TS);
        assert!(resp.contains("No messages"));

        // KD3BFP-9 connecting and listing sees the message.
        let (resp, _) = handle_line(&mut state, &mut messages, "KD3BFP-9", "L", TS);
        assert!(resp.contains("Test Subject"));

        let id = messages[0].id;
        let (resp, _) = handle_line(&mut state, &mut messages, "KD3BFP-9", &format!("R {id}"), TS);
        assert!(resp.contains("line one"));
        assert!(messages[0].read);
    }

    #[test]
    fn bye_signals_close() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;
        let (_, close) = handle_line(&mut state, &mut messages, "N0CALL-1", "B", TS);
        assert!(close);
    }

    #[test]
    fn unknown_command_reprompts_without_closing() {
        let mut messages = Vec::new();
        let mut state = MailboxState::Command;
        let (resp, close) = handle_line(&mut state, &mut messages, "N0CALL-1", "ZZZ", TS);
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
        // The mailbox never falls back to any general/default callsign --
        // it requires its own explicit one (also enforced up front by the
        // Enable button, which refuses to turn on without one).
        assert!(!should_answer(true, "", Some("KD3BFP-1")));
        assert!(!should_answer(true, "", None));
    }

    #[test]
    fn should_answer_only_matches_configured_callsign() {
        // Exact scenario from the feature request: mailbox configured to
        // respond as KD3BFP-5 must not answer a connect addressed to a
        // different SSID of the same base callsign.
        assert!(should_answer(true, "KD3BFP-5", Some("KD3BFP-5")));
        assert!(!should_answer(true, "KD3BFP-5", Some("KD3BFP-6")));
        // Case-insensitive, and tolerant of incidental whitespace in the
        // configured value.
        assert!(should_answer(true, " kd3bfp-5 ", Some("KD3BFP-5")));
        // No known destination (a backend that doesn't report `to`) can't be
        // confirmed as addressed to the configured call, so it's declined
        // rather than assumed.
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
