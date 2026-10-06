//! The settings editor's colours and checks: ini and JSON are coloured as they are typed, and simple mistakes are listed under the text.

use crate::theme::{GREEN, ORANGE, PURPLE, SECONDARY, TEAL_TEXT};
use eframe::egui::{text::LayoutJob, Color32, FontId, TextFormat};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Syntax {
    Ini,
    Json,
    Plain,
}

pub fn syntax_of(name: &str) -> Syntax {
    let lower = name.to_lowercase();
    if lower.ends_with(".json") {
        Syntax::Json
    } else if [".ini", ".cfg", ".conf", ".toml", ".properties"].iter().any(|e| lower.ends_with(e)) {
        Syntax::Ini
    } else {
        Syntax::Plain
    }
}

const PLAIN: Color32 = Color32::from_rgb(230, 230, 235);
const SECTION: Color32 = Color32::from_rgb(110, 175, 255);
const PLACEHOLDER: Color32 = Color32::from_rgb(255, 214, 102);

fn push(job: &mut LayoutJob, text: &str, color: Color32, font: &FontId) {
    if !text.is_empty() {
        job.append(text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
    }
}

/// A value: numbers, booleans and `{placeholders}` get their own colours.
fn push_value(job: &mut LayoutJob, v: &str, font: &FontId) {
    let t = v.trim();
    let lower = t.to_lowercase();
    if !t.is_empty() && t.parse::<f64>().is_ok() {
        push(job, v, ORANGE, font);
    } else if matches!(lower.as_str(), "true" | "false" | "yes" | "no" | "on" | "off") {
        push(job, v, PURPLE, font);
    } else if v.contains('{') {
        let mut rest = v;
        while let Some(a) = rest.find('{') {
            let Some(b) = rest[a..].find('}') else { break };
            push(job, &rest[..a], GREEN, font);
            push(job, &rest[a..a + b + 1], PLACEHOLDER, font);
            rest = &rest[a + b + 1..];
        }
        push(job, rest, GREEN, font);
    } else {
        push(job, v, GREEN, font);
    }
}

fn ini(job: &mut LayoutJob, text: &str, font: &FontId) {
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let nl = &line[body.len()..];
        let t = body.trim_start();
        if t.starts_with(';') || t.starts_with('#') || t.starts_with("//") {
            push(job, body, SECONDARY, font);
        } else if t.starts_with('[') {
            push(job, body, SECTION, font);
        } else if let Some(eq) = body.find('=') {
            let (key, rest) = body.split_at(eq);
            push(job, key, TEAL_TEXT, font);
            push(job, "=", SECONDARY, font);
            // a comment after the value
            let value = &rest[1..];
            match value.find(" ;").or_else(|| value.find(" #")) {
                Some(c) => {
                    push_value(job, &value[..c], font);
                    push(job, &value[c..], SECONDARY, font);
                }
                None => push_value(job, value, font),
            }
        } else {
            push(job, body, PLAIN, font);
        }
        push(job, nl, PLAIN, font);
    }
}

fn json(job: &mut LayoutJob, text: &str, font: &FontId) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let s = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            // a key is a string followed by ':'
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let is_key = j < chars.len() && chars[j] == ':';
            push(job, &s(start, i), if is_key { TEAL_TEXT } else { GREEN }, font);
        } else if c == '-' || c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || "+-.eE".contains(chars[i])) {
                i += 1;
            }
            push(job, &s(start, i), ORANGE, font);
        } else if c.is_ascii_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            push(job, &s(start, i), PURPLE, font);
        } else {
            let start = i;
            while i < chars.len() && !(chars[i] == '"' || chars[i] == '-' || chars[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            push(job, &s(start, i), SECONDARY, font);
        }
    }
}

/// The coloured layout of the text.
pub fn highlight(text: &str, syntax: Syntax, font: &FontId, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    match syntax {
        Syntax::Ini => ini(&mut job, text, font),
        Syntax::Json => json(&mut job, text, font),
        Syntax::Plain => push(&mut job, text, PLAIN, font),
    }
    job.wrap.max_width = wrap_width;
    job
}

/// Mistakes worth saying: (line number from 1, what is wrong). `messages` are the window's words for them.
pub fn lint(text: &str, syntax: Syntax, messages: &LintWords) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    match syntax {
        Syntax::Json => {
            if !text.trim().is_empty() {
                if let Err(e) = serde_json::from_str::<serde_json::Value>(text) {
                    out.push((e.line(), format!("{} ({e})", messages.json)));
                }
            }
        }
        Syntax::Ini => {
            let mut section = String::new();
            let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
            for (n, line) in text.lines().enumerate() {
                let t = line.trim();
                if t.is_empty() || t.starts_with(';') || t.starts_with('#') || t.starts_with("//") {
                    continue;
                }
                if t.starts_with('[') {
                    if !t.ends_with(']') {
                        out.push((n + 1, messages.section.clone()));
                    }
                    section = t.trim_matches(['[', ']']).trim().to_lowercase();
                    continue;
                }
                match t.split_once('=') {
                    None => out.push((n + 1, messages.no_equals.clone())),
                    Some((k, _)) if k.trim().is_empty() => out.push((n + 1, messages.no_key.clone())),
                    Some((k, _)) => {
                        if !seen.insert((section.clone(), k.trim().to_lowercase())) {
                            out.push((n + 1, messages.duplicate.replace("{0}", k.trim())));
                        }
                    }
                }
            }
        }
        Syntax::Plain => {}
    }
    out
}

pub struct LintWords {
    pub json: String,
    pub section: String,
    pub no_equals: String,
    pub no_key: String,
    /// `{0}` is the key
    pub duplicate: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> LintWords {
        LintWords { json: "json".into(), section: "section".into(), no_equals: "no =".into(), no_key: "no key".into(), duplicate: "dup {0}".into() }
    }

    #[test]
    fn ini_mistakes_are_found_by_line() {
        let text = "[a]\nx=1\nnonsense\n=3\nX=2\n[b\n; ok\n[c]\nx=1\n";
        let found = lint(text, Syntax::Ini, &words());
        assert_eq!(found, vec![(3, "no =".into()), (4, "no key".into()), (5, "dup X".into()), (6, "section".into())]);
    }

    #[test]
    fn json_errors_name_the_line() {
        let found = lint("{\n  \"a\": 1,\n  \"b\" 2\n}", Syntax::Json, &words());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 3);
        assert!(lint("{\"a\": [1, true, null]}", Syntax::Json, &words()).is_empty());
    }

    #[test]
    fn highlighting_keeps_every_character() {
        let font = FontId::monospace(13.0);
        for (text, syntax) in [("[s]\nk = {plugin_dir}\\x ; c\nn=3\n", Syntax::Ini), ("{\"k\": [1, -2.5e3, true, \"s\\\"q\"]}", Syntax::Json), ("中文\n", Syntax::Plain)] {
            assert_eq!(highlight(text, syntax, &font, 100.0).text, text);
        }
        assert_eq!(syntax_of("A.INI"), Syntax::Ini);
        assert_eq!(syntax_of("x.json"), Syntax::Json);
    }
}
