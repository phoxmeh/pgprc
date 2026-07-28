//! Line-based scrollback highlighting shared by the Monitor view and every
//! session tab: colors AX.25-style callsigns (known ones from the address
//! book differently), the bracketed frame/command tag on monitor lines
//! (`[UI]`, `[SABM]`, `[unproto TX]`, ...), and user-configurable keyword/
//! bulletin rules (`HighlightRule` in pr-core).
//!
//! Rebuilt fresh from the current `AppConfig` on every line rather than
//! cached: packet radio traffic runs at a few lines per second at most, so
//! recompiling a handful of small regexes per line is free, and it sidesteps
//! any cache-invalidation bugs when Preferences or the address book change.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gtk::prelude::*;
use regex::{Regex, RegexBuilder};

use pr_core::AppConfig;

use crate::qrz::strip_ssid;

/// Compiled, ready-to-scan form of the user's highlighting preferences.
pub struct Highlighter {
    enabled: bool,
    callsign_re: Regex,
    callsign_color: String,
    known_color: String,
    known_bases: HashSet<String>,
    my_call_color: String,
    my_call_base: Option<String>,
    ax25_re: Regex,
    ax25_color: String,
    /// Custom/keyword rules in configured order; later entries are applied
    /// (and thus visually layered) after earlier ones.
    rules: Vec<(Regex, String)>,
}

impl Highlighter {
    pub fn build(config: &AppConfig) -> Self {
        let hl = &config.highlighting;
        let known_bases = config
            .address_book
            .iter()
            .map(|e| strip_ssid(&e.callsign).to_uppercase())
            .collect();
        let my_call_base = config
            .ui
            .default_call
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| strip_ssid(s).to_uppercase());
        let rules = hl
            .rules
            .iter()
            .filter(|r| r.enabled)
            .filter_map(|r| {
                let pattern = if r.regex {
                    r.pattern.clone()
                } else {
                    let alts: Vec<String> = r
                        .pattern
                        .split([',', '|'])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(regex::escape)
                        .collect();
                    if alts.is_empty() {
                        return None;
                    }
                    format!(r"\b({})\b", alts.join("|"))
                };
                RegexBuilder::new(&pattern)
                    .case_insensitive(true)
                    .build()
                    .ok()
                    .map(|re| (re, r.color.clone()))
            })
            .collect();

        Highlighter {
            enabled: hl.enabled,
            // 1-2 letters, a digit, 1-4 letters, optional SSID — standard
            // amateur callsign shape (a heuristic, not a strict validator).
            callsign_re: RegexBuilder::new(r"\b[A-Za-z]{1,2}[0-9][A-Za-z]{1,4}(-[0-9]{1,2})?\b")
                .build()
                .expect("static regex"),
            callsign_color: hl.callsign_color.clone(),
            known_color: hl.known_callsign_color.clone(),
            known_bases,
            my_call_color: hl.my_call_color.clone(),
            my_call_base,
            // Matches only the bracketed frame/command tag itself (exact
            // content, not a generic "any bracketed text" scan) — otherwise
            // this would also light up unrelated bracketed labels like
            // `[port-1]` or a port's own `[Direwolf]` name prefix.
            ax25_re: Regex::new(
                r"\[(?:[USTCDdGRHg]|UI|SABM|DISC|DM|UA|FRMR|unproto TX|I N\(S\)=\d+ N\(R\)=\d+|RR N\(R\)=\d+|RNR N\(R\)=\d+|REJ N\(R\)=\d+)\]",
            )
            .expect("static regex"),
            ax25_color: hl.ax25_command_color.clone(),
            rules,
        }
    }

    /// Byte-offset `(start, end, color)` spans within `line` to colorize.
    /// Overlaps are possible (e.g. a keyword inside a callsign token isn't
    /// prevented); GTK resolves overlapping foreground tags by the tag's
    /// table priority, which `TagCache` assigns in first-seen order.
    fn spans(&self, line: &str) -> Vec<(usize, usize, String)> {
        if !self.enabled {
            return Vec::new();
        }
        let mut spans = Vec::new();
        for m in self.ax25_re.find_iter(line) {
            spans.push((m.start(), m.end(), self.ax25_color.clone()));
        }
        for m in self.callsign_re.find_iter(line) {
            let base = strip_ssid(m.as_str()).to_uppercase();
            let color = if self.my_call_base.as_deref() == Some(base.as_str()) {
                self.my_call_color.clone()
            } else if self.known_bases.contains(&base) {
                self.known_color.clone()
            } else {
                self.callsign_color.clone()
            };
            spans.push((m.start(), m.end(), color));
        }
        for (re, color) in &self.rules {
            for m in re.find_iter(line) {
                spans.push((m.start(), m.end(), color.clone()));
            }
        }
        spans
    }
}

/// Per-buffer cache of `color -> TextTag`, so repeated matches of the same
/// color reuse one tag instead of growing the buffer's tag table forever.
#[derive(Default)]
pub struct TagCache {
    tags: RefCell<HashMap<String, gtk::TextTag>>,
}

impl TagCache {
    pub fn new() -> Self {
        TagCache::default()
    }

    fn get_or_create(&self, buffer: &gtk::TextBuffer, color: &str) -> gtk::TextTag {
        if let Some(tag) = self.tags.borrow().get(color) {
            return tag.clone();
        }
        let tag = gtk::TextTag::builder().foreground(color).build();
        buffer.tag_table().add(&tag);
        self.tags.borrow_mut().insert(color.to_string(), tag.clone());
        tag
    }
}

/// Apply highlighting to `line_text`, which must already sit in `buffer` at
/// `[line_start_offset, line_start_offset + line_text.chars().count())`
/// (a char offset, not byte offset — GTK `TextIter` positions are in chars).
pub fn highlight_line(highlighter: &Highlighter, buffer: &gtk::TextBuffer, tags: &TagCache, line_start_offset: i32, line_text: &str) {
    for (byte_start, byte_end, color) in highlighter.spans(line_text) {
        let char_start = line_text[..byte_start].chars().count() as i32;
        let char_end = line_text[..byte_end].chars().count() as i32;
        let tag = tags.get_or_create(buffer, &color);
        let start_iter = buffer.iter_at_offset(line_start_offset + char_start);
        let end_iter = buffer.iter_at_offset(line_start_offset + char_end);
        buffer.apply_tag(&tag, &start_iter, &end_iter);
    }
}
