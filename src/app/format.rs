//! Pure formatting helpers extracted from `app/mod.rs` (#743).
//!
//! No I/O, no app state — pure text/number transformations. `trunc` builds on
//! `quadraui::text_util::char_cell_width` (#5) for correct terminal-column
//! measurement rather than reimplementing width tables; `fuzzy_score` and
//! `word_wrap` used to live here too but were deleted in favour of
//! `quadraui::text_util::{fuzzy_score, word_wrap}` directly at call sites
//! (#5) — see that module for the current implementations.
use std::time::{SystemTime, UNIX_EPOCH};

/// Whole days, in seconds — the unit [`format_unix_time`] rolls up to once a
/// timestamp's age passes 24h.
const SECS_PER_DAY: u64 = 86_400;

pub(crate) fn fmt_dur(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Format elapsed seconds with an explicit unit on every segment, so the
/// under-an-hour and over-an-hour regimes can never be confused for one
/// another: `2m22s` (< 1 hour) vs `2h22m` (≥ 1 hour).
///
/// #2397: the prior `fmt_elapsed_mmss` rendered both regimes as the same
/// bare `N:NN` shape (`M:SS` under an hour, `H:MM` at/above it) — "2:22" read
/// identically whether it meant 2 minutes or 2 hours 22 minutes, which is
/// exactly what made a stalled-merge "waiting 2:22" box undiagnosable live
/// (issue #2397's incident writeup). Every call site switched to this
/// helper together so no ambiguous rendering survives anywhere in the TUI.
pub(crate) fn fmt_elapsed(secs: u64) -> String {
    if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// #2397: classify a merge-queue block reason (`coord.merge_queue.plan()`'s
/// per-entry `reason`, reaching the TUI as `PlannedMergeEntry.reason`) into
/// the "should I be worried" affordance an operator reading a Pending Merge
/// stage needs — today they have to run `coord merge --plan` by hand to get
/// this (docs/MERGE_AUTO_DRAIN_TRUST_BAR.md; live incidents #2284/#2375).
///
/// Three buckets:
/// - **in flight**: `reason` starts with `coord.merge_queue.CI_PENDING_PREFIX`
///   ("CI running:") — a check is executing *right now*; this resolves on
///   its own with no human step.
/// - **needs a person**: `reason` names a gate that no amount of retrying
///   clears by itself — an unapproved/rejected review, failed checks, or a
///   real merge conflict. Matched against the same wording
///   `coord.merge_queue._entry_gate_status` emits for those cases (mirrored
///   here as prose classification for *display* only — no gate re-evaluation
///   happens in Rust).
/// - **waiting on `coord merge`**: everything else (typically a stale-but-
///   green CI check, `CI_STALE_PREFIX`, or a stale/missing smoke verdict,
///   `smoke_required`) — nothing retries this on its own, ever.
///
/// #2397 review fix: this deliberately does **not** take a `merge.auto_drain`
/// flag. An earlier version of this function branched on `auto_drain` to
/// render "auto-retry armed" for the waiting bucket, which was factually
/// wrong: `serve_app._auto_drain_tick` restricts every retry it performs
/// (including the #2197 stale-CI auto-rerun) to entries `coord.merge_queue
/// .plan()` already marked `PLAN_READY` — a `PLAN_BLOCKED` entry (which is
/// what every reason reaching this function represents; `reason` is `None`
/// for READY entries and `fmt_merge_block_reason` never calls this then) is
/// filtered out *before* `_auto_drain_tick` ever calls `process()` on it, so
/// `auto_drain` being `true` changes nothing about whether a BLOCKED entry
/// gets retried. It stays blocked until a human runs `coord merge` (or
/// `coord merge --revalidate`) regardless of the flag. See
/// `coord.serve_app._auto_drain_tick`'s docstring: "`BLOCKED`… entries are
/// never touched."
pub(crate) fn merge_wait_affordance(reason: &str) -> &'static str {
    const NEEDS_PERSON_MARKERS: [&str; 5] = [
        "CI failed:",
        "review not approved",
        "conflict",
        "CI never ran:",
        "CI unreadable:",
    ];
    if reason.starts_with("CI running:") {
        "auto-retry in flight — CI re-checking now"
    } else if NEEDS_PERSON_MARKERS.iter().any(|m| reason.contains(m)) {
        "blocked — needs a person to resolve"
    } else {
        "waiting on a human — nothing retries until `coord merge` runs"
    }
}

/// #2402: mirrors `coord.merge_queue.revalidation_candidates` /
/// `ci_revalidation_candidates`'s eligibility rule **exactly** — this is the
/// predicate that gates the Pipeline row's "Re-verify (revalidate)"
/// context-menu action, not merely a display classification. A BLOCKED
/// Merge stage's raw `reason` (`PlannedMergeEntry.reason`, the wording
/// `coord.merge_queue._entry_gate_status` emits) is revalidate-eligible only
/// when it is one of:
///
/// - `"test verdict stale ("` — `SmokeVerdictStatus.short_reason` for a
///   `SMOKE_STALE` verdict (a passed verdict recorded against a base/branch/
///   run that has since moved). `revalidation_candidates` requires this to
///   be the *only* gate failure on the entry — a `SMOKE_MISSING` verdict
///   ("test verdict missing", nothing was ever recorded) is deliberately
///   excluded there, and must be excluded here too: a re-test can't conjure
///   up a verdict that was never taken.
/// - `"CI stale:"` (`coord.merge_queue.CI_STALE_PREFIX`) — a passing check
///   list that predates the current base HEAD. `ci_revalidation_candidates`'s
///   whole eligibility test.
///
/// Everything else — an unapproved/rejected review, a real CI failure, an
/// unreadable CI fetch, or a genuine merge conflict — is a block no re-test
/// can clear. `merge_wait_affordance` above lumps `SMOKE_MISSING` in with
/// these two staleness cases for *display* purposes (its coarser "waiting on
/// a human" bucket), which is why this is a separate, stricter predicate
/// rather than a reuse of that one: an action gate has to be exact, a
/// display classification only has to be roughly right.
pub(crate) fn merge_revalidate_eligible(reason: &str) -> bool {
    reason.starts_with("test verdict stale (") || reason.starts_with("CI stale:")
}

/// #2397: `"{reason} [{affordance}]"`, truncated so a verbose gate-failure
/// reason (e.g. a multi-check CI failure summary) can't blow out a stage
/// box's width — the affordance tag is the load-bearing part for the
/// "should I be worried" read, so it's kept intact and the reason prose is
/// trimmed instead. `None` in, `None` out (no plan entry / not blocked).
pub(crate) fn fmt_merge_block_reason(reason: Option<&str>) -> Option<String> {
    let reason = reason?;
    let affordance = merge_wait_affordance(reason);
    Some(format!("{} [{}]", trunc(reason, 80), affordance))
}

/// Capitalize the first ASCII character of `s` (no-op when `s` is empty
/// or starts with a non-ASCII character).
pub(crate) fn capitalize(s: &str) -> String {
    let mut out = s.to_string();
    if let Some(c) = out.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    out
}

/// Format a unix timestamp as a relative "Xs/m/h ago" string using
/// the existing `fmt_dur` helper.  Falls back to "-" when the
/// timestamp is in the future or the system clock can't be read.
/// (#818: previously used by the Stages tab detail rows; #1022: now
/// used by the Pipeline Summary tab to show relative completion times.)
pub(crate) fn format_unix_time(ts: f64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let delta = (now - ts).max(0.0) as u64;
    // `fmt_dur`'s largest unit is the hour, which is fine for a *duration*
    // (where the extra precision is the point) but unbounded for an *age*:
    // a year-old row rendered `9732h35m ago` — 12 columns of mostly noise.
    // The Audit table's Time column is `ColumnWidth::Fixed(11.0)` (10 usable
    // cells, see `audit_columns`), so `draw_data_table` clipped that to
    // `9732h35m …` and ate the `ago` suffix entirely, breaking #1039
    // contract §4a ("the relative-time column ... must contain `ago`").
    //
    // Rolling up to whole days keeps the string relative (§4a) *and* inside
    // the budget: `406d ago` is 8 cells, and even a 5-digit day count fits.
    // Days-only rather than `{d}d{h}h` is deliberate — `406d12h ago` is 11
    // cells and would clip exactly the same way.
    //
    // Scoped to this helper on purpose: `fmt_dur` itself is left alone so
    // the duration call sites (stage elapsed, `Duration` detail rows) keep
    // their hour+minute precision. Ages get coarse units; durations do not.
    if delta >= SECS_PER_DAY {
        return format!("{}d ago", delta / SECS_PER_DAY);
    }
    format!("{} ago", fmt_dur(delta))
}

/// Inverse of Howard Hinnant's `days_from_civil`: days-since-the-Unix-epoch
/// → `(year, month, day)` in the proleptic Gregorian calendar, UTC.
/// <http://howardhinnant.github.io/date_algorithms.html>
///
/// This workspace carries no chrono/time crate, so the one place that needs
/// an absolute calendar rendering ([`format_unix_abs`]) has to do the
/// arithmetic itself. It lived in `app/usage.rs` until #1763 retired that
/// panel — the *server* now owns every calendar decision the Usage view
/// used to make locally (Today/Week/Month boundaries are `datetime` in
/// `coord/usage_rollup.py`), and all that survives client-side is turning
/// one epoch float into `YYYY-MM-DD` for display.
///
/// `div_euclid` (not `/`) is required for the `era` step: `z` can be
/// negative there and Rust's `/` truncates toward zero, but the algorithm
/// needs floor division.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// #1762: absolute UTC rendering of a unix timestamp — `YYYY-MM-DD HH:MM`.
pub(crate) fn format_unix_abs(ts: f64) -> String {
    const SECS_PER_DAY: f64 = 86_400.0;
    // `.floor()`, not a truncating cast: a pre-1970 timestamp is negative
    // and must round *down* to its day, or the date lands a day late and
    // the seconds-into-the-day remainder goes negative.
    let days = (ts / SECS_PER_DAY).floor();
    let rem = (ts - days * SECS_PER_DAY).max(0.0) as u64;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02} {h:02}:{min:02}",
        h = (rem / 3600) % 24,
        min = (rem % 3600) / 60
    )
}

/// #1765: a unix timestamp as a filename-safe UTC stamp — `YYYYMMDD-HHMM`.
///
/// Deliberately the same `%Y%m%d-%H%M` the server's `coord.reports.
/// csv_filename` produces, so the panel's save-dialog suggestion and the
/// daemon's `Content-Disposition` name agree for the same run. Derived from
/// [`format_unix_abs`] rather than re-deriving the calendar arithmetic —
/// one civil-date implementation, not two.
pub(crate) fn format_unix_stamp(ts: f64) -> String {
    let abs = format_unix_abs(ts); // "YYYY-MM-DD HH:MM"
    match abs.split_once(' ') {
        Some((date, time)) => format!("{}-{}", date.replace('-', ""), time.replace(':', "")),
        None => abs.replace('-', ""),
    }
}

/// Timestamps older (or newer) than this render absolutely rather than
/// relatively. `fmt_dur`'s largest unit is the hour, so beyond a couple of
/// days "72h0m ago" is strictly less legible than a date — and a report
/// window can reach back weeks.
const RELATIVE_TIME_MAX_SECS: f64 = 48.0 * 3600.0;

/// #1762: a `timestamp` cell's rendering — relative while recent
/// (`13h ago`), absolute otherwise (`2026-07-28 14:03`).
///
/// A future timestamp always renders absolutely: [`format_unix_time`]
/// clamps the delta at zero, so a clock-skewed row would otherwise claim
/// `0s ago` no matter how far ahead it actually is.
pub(crate) fn format_unix_smart(ts: f64) -> String {
    if !ts.is_finite() {
        return String::new();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let delta = now - ts;
    if (0.0..RELATIVE_TIME_MAX_SECS).contains(&delta) {
        format_unix_time(ts)
    } else {
        format_unix_abs(ts)
    }
}

/// #546: format a token count with K/M suffix and one decimal place.
///
/// Examples: 1500 → "1.5k", 2_300_000 → "2.3M", 800 → "800".
/// Used to keep token counts readable in the narrow TUI columns.
pub(crate) fn fmt_tokens(n: i64) -> String {
    if n <= 0 {
        return "0".to_string();
    }
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// #208: format a worker cost in USD with two decimals.  Below 1¢ shows
/// "< $0.01" so the rendering doesn't read as $0.00 (mathematically true
/// but misleading — the worker did some non-zero work).
pub(crate) fn format_cost_usd(cost: f64) -> String {
    if cost <= 0.0 {
        "$0.00".to_string()
    } else if cost < 0.01 {
        "< $0.01".to_string()
    } else {
        format!("${cost:.2}")
    }
}

/// #1763: render a `money`-kind report cell (per `column_meta`, #1760).
///
/// Four decimal places so $0.9060 is distinguishable from $0.91 — a single
/// worker leg genuinely costs fractions of a cent, and `$0.00` would read as
/// free. Exact zero renders as "—" rather than `$0.0000`: in the `usage`
/// report every dollar column has a companion (captured/estimated/total), so
/// a blank cell unambiguously means "nothing here" instead of implying a
/// figure was computed and came out at zero.
///
/// This is the *generic* renderer for the `money` kind, not a Usage-specific
/// helper: it is reached only through `reports_cell_text`'s dispatch on
/// `kind`, so any report that declares a `money` column gets it. (It began
/// as #1116's `format_cost_captured` for the retired Usage panel.)
pub(crate) fn format_money(cost: f64) -> String {
    if cost == 0.0 {
        "—".to_string()
    } else {
        format!("${cost:.4}")
    }
}

/// #1763: render a `duration`-kind report cell as a compact `NmSSs` /
/// `NhMMmSSs` (e.g. `"45m00s"`), distinct from `fmt_dur`'s coarser
/// `"45m"`/`"1h30m"` used for elapsed-time display. A report duration is
/// typically a *sum* over many legs, so seconds-precision avoids a short
/// total reading as `"0m"` / "no data". `secs <= 0.0` renders as "—".
pub(crate) fn format_duration_compact(secs: f64) -> String {
    if secs <= 0.0 {
        return "—".to_string();
    }
    let total = secs.round() as u64;
    let (h, rem) = (total / 3600, total % 3600);
    let (m, s) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else {
        format!("{m}m{s:02}s")
    }
}

/// Collapse all runs of whitespace (including newlines and tabs) into single
/// spaces and trim the ends. Used to render a multi-line assistant text block
/// as one horizontally-scrollable Log row (#302) without embedded newlines
/// breaking the single-line list item.
pub(crate) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate `s` to at most `max_cols` *terminal display columns* (not
/// `char`s — a wide glyph like `🔍`/`🎭` (sidebar action icons, sidebar.rs
/// action-icon table) occupies two columns, so measuring with
/// `.chars().count()` under-counts it by half and mis-truncates by a
/// column; see #5).
///
/// Built on [`quadraui::text_util::char_cell_width`] rather than
/// reimplementing Unicode width tables. Always cuts on a `char` boundary and
/// never splits a double-width glyph's two columns — if the next character
/// would overflow the budget it is dropped whole, even if one column of
/// budget remains. Mirrors `quadraui::tui::text::truncate_to_width`'s
/// algorithm exactly, but reimplemented here on the always-available
/// `text_util` primitive since `quadraui::tui` is gated behind the crate's
/// own `tui` feature (off for a `--no-default-features --features gtk`
/// build) and this helper is called from feature-independent app logic.
pub(crate) fn trunc(s: &str, max_cols: usize) -> &str {
    let mut used = 0usize;
    for (idx, c) in s.char_indices() {
        let w = quadraui::text_util::char_cell_width(c) as usize;
        if used + w > max_cols {
            return &s[..idx];
        }
        used += w;
    }
    s
}

/// Wrap `s` in `"  (…)"`-style parens for a merge-queue row label,
/// truncating to `max_cols` display columns with a trailing "…" when it
/// doesn't fit. Built on [`trunc`], so this is char/display-width safe.
///
/// #11: the two merge-queue label call sites (`merge_queue_entry_label`'s
/// gate/conflict reason and `render_merge_plan_panel`'s BLOCKED-entry
/// reason) used to slice with a literal byte index (`&s[..57]`, `&s[..47]`)
/// directly into arbitrary git/gh error text. That panics whenever a
/// multi-byte character (smart quotes, non-ASCII filenames, box-drawing —
/// exactly the class already documented at `first_meaningful_stderr_line`'s
/// #1381-1385 history) lands on the cut boundary. Centralizing the pattern
/// here means the fix (and its regression test) lives in one place instead
/// of being duplicated — and re-copied incorrectly — at every call site.
pub(crate) fn fmt_truncated_paren(s: &str, max_cols: usize) -> String {
    let cut = trunc(s, max_cols);
    if cut.len() < s.len() {
        format!("  ({}…)", cut)
    } else {
        format!("  ({})", s)
    }
}
