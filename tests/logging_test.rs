//! Guards the structured-logging convention across the whole crate.
//!
//! `RDRS_LOG_FORMAT=json` is only useful if every event carries fields an
//! operator can filter and aggregate on. Before this guard existed, one module
//! (`services/audit.rs`) emitted proper fields and the other ~70 call sites
//! interpolated everything into the message string — `warn!("Feed {} sync
//! failed: {}", feed_id, e)` collapses to an opaque `message` with no
//! `feed_id`. That is the drift this test exists to prevent: it is a lint over
//! the source, not a runtime assertion, because the failure mode is a *new*
//! call site written the old way, which no runtime test would exercise.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `tracing` log macro invocation found under `src/`, as (file, line,
/// argument text).
fn log_macro_calls() -> Vec<(PathBuf, usize, String)> {
    let mut calls = Vec::new();
    let mut files = Vec::new();
    collect_rs_files(Path::new("src"), &mut files);
    assert!(!files.is_empty(), "no Rust sources found under src/");

    for path in files {
        let source = fs::read_to_string(&path).expect("source file must be readable");
        // Byte index throughout — `source.chars().count()` would stop the scan
        // early in every file containing a non-ASCII character (these comments
        // are full of em dashes), silently truncating the audit.
        let mut idx = 0;
        while idx < source.len() {
            let Some(open) = find_macro_open(&source, idx) else {
                break;
            };
            let Some(close) = matching_paren(&source, open) else {
                break;
            };
            let args = source[open + 1..close].to_string();
            let line = source[..open].matches('\n').count() + 1;
            calls.push((path.clone(), line, args));
            idx = close + 1;
        }
    }
    // A parser bug that found nothing would make both assertions below pass
    // vacuously. The crate had 101 log calls when this guard was written; a
    // floor well under that catches a broken scan without tripping every time
    // a call site is added or removed.
    assert!(
        calls.len() >= 80,
        "only {} log calls found under src/ — the scanner is probably broken, \
         which would make the assertions below pass vacuously",
        calls.len()
    );
    calls
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Byte index of the `(` opening the next `info!`/`warn!`/… invocation at or
/// after `from`. A `tracing::` prefix is irrelevant — the match anchors on the
/// macro name itself.
fn find_macro_open(source: &str, from: usize) -> Option<usize> {
    const LEVELS: [&str; 5] = ["info!(", "warn!(", "error!(", "debug!(", "trace!("];
    let mut best: Option<usize> = None;
    for level in LEVELS {
        // Every occurrence, not just the first: skipping one that turns out to
        // be part of a longer identifier must not blind us to real calls after
        // it (`my_warn!(…)` would otherwise hide the next genuine `warn!`).
        let mut search = from;
        while let Some(rel) = source[search..].find(level) {
            let at = search + rel;
            search = at + level.len();
            // Require the char before the macro name to not continue an
            // identifier, so `my_warn!(` does not read as `warn!(`.
            if source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let open = at + level.len() - 1;
            best = Some(best.map_or(open, |b: usize| b.min(open)));
            break;
        }
    }
    best
}

/// Byte index of the `)` matching the `(` at `open`, ignoring parens inside
/// string literals.
fn matching_paren(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in source.char_indices().skip_while(|(i, _)| *i < open) {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every log call must carry an `event = "domain.verb"` field so JSON output
/// can be filtered by event rather than by substring-matching the message.
///
/// `services/audit.rs` is not exempt — it already sets `event` on every call,
/// alongside `target: AUDIT_TARGET`.
#[test]
fn every_log_call_carries_an_event_field() {
    let offenders: Vec<String> = log_macro_calls()
        .into_iter()
        .filter(|(_, _, args)| !args.contains("event = "))
        .map(|(path, line, args)| {
            let preview: String = args.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(
                "{}:{line}: {}",
                path.display(),
                &preview[..preview.len().min(80)]
            )
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these log calls have no `event = \"domain.verb\"` field, so JSON logs \
         cannot be filtered by event:\n{}",
        offenders.join("\n")
    );
}

/// The message must be a static string. Interpolating values into it is what
/// hides them from JSON output — the whole point of the field convention.
///
/// The two `config.warning` sites are the deliberate exception: their message
/// *is* the payload (a prose warning assembled in `config.rs`), there is no
/// structure to lift out, and they carry a `kind` field to tell them apart.
#[test]
fn log_messages_do_not_interpolate_values() {
    let offenders: Vec<String> = log_macro_calls()
        .into_iter()
        .filter(|(_, _, args)| !args.contains("config.warning"))
        .filter(|(_, _, args)| {
            // The message is the last string literal in the argument list. A
            // `{...}` inside it means a value was formatted into the message.
            args.rfind('"')
                .and_then(|end| args[..end].rfind('"').map(|start| &args[start..=end]))
                .is_some_and(|msg| msg.contains('{'))
        })
        .map(|(path, line, args)| {
            let preview: String = args.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(
                "{}:{line}: {}",
                path.display(),
                &preview[..preview.len().min(80)]
            )
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these log calls interpolate values into the message instead of \
         attaching them as fields:\n{}",
        offenders.join("\n")
    );
}
