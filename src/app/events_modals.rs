//! Modal / dialog keyboard intercepts lifted out of [`CoordApp::dispatch_handle`] (#13).
//!
//! `dispatch_handle` grew to ~4,700 lines, which made it this crate's #1
//! merge-conflict site: every keybind change edited the same function. #13's
//! recommended decomposition is to hoist each overlay's key handling into an
//! early-return `-> Option<Reaction>` helper; this module is that hoist for the
//! *blocking modal* tier (confirms, pickers, single-field prompts, info dialogs,
//! the context menu).
//!
//! **These are pure code motion.** Each helper is the original `if
//! self.pending_x.is_some() { … }` block verbatim, with `return Reaction::R`
//! rewritten to `return Some(Reaction::R)` and a trailing `None` meaning "not
//! mine — fall through". `dispatch_handle` calls them in the same order the
//! inlined blocks ran, and **that order is load-bearing**: when two modals are
//! alive at once (the PTY-panic dialog over an artifact-pull dialog, say) the
//! first block to claim the key wins, matching `build_prompt_dialog`'s render
//! priority. Do not reorder these calls or regroup the blocks across helper
//! boundaries without checking that pairing.
//!
//! **Import pattern:** `use super::*` — same rationale as `events.rs`: these are
//! `CoordApp` methods and need the full parent namespace.
#[allow(unused_imports)]
use super::*;

impl CoordApp {
    /// #722: an offer whose issue still has a live interactive session must
    /// survive Esc — the operator is expected to reattach and `/exit` first, at
    /// which point the offer re-fires automatically. Returns `true` (having
    /// toasted) when the offer must be kept rather than dismissed.
    ///
    /// `noun` names the offer in the toast ("review" / "stage" / "fix" /
    /// "merge" / "force-fix"); the five call sites in
    /// [`Self::handle_stage_offer_keys`] were byte-identical apart from that
    /// one word, so #13 folded them into this helper.
    fn offer_pinned_by_live_session(
        &mut self,
        issue_num: u64,
        coord_repo: &str,
        noun: &str,
    ) -> bool {
        if !self.issue_has_live_session_for_repo(issue_num, coord_repo) {
            return false;
        }
        self.push_toast(
            "Reattach first",
            &format!(
                "Close the live session for #{issue_num} first; \
                 the {noun} offer will re-appear automatically.",
            ),
            ToastSeverity::Warning,
        );
        true
    }

    /// Post-stage one-key offers (auto-review, next-stage, rework, test→fix,
    /// test→merge, force-past-cap).
    ///
    /// Runs BEFORE the detail-terminal focus arbitration so a still-focused
    /// shell — the one that just ran `coord assign` — cannot eat the Enter.
    /// Unlike the modals below these are *non-blocking* offers: keys they do
    /// not claim fall through so the shell underneath stays usable.
    pub(crate) fn handle_stage_offer_keys(&mut self, event: &UiEvent) -> Option<Reaction> {
        // ── Test → Review confirm (Test precedes Review) ─────────────────
        // When the smoke test passes (board-driven, never scraped) the
        // detector raises `pending_auto_review`.  Own Enter (confirm →
        // launch the interactive review) and Esc/n (dismiss) here, BEFORE
        // the detail-terminal focus block — otherwise a still-focused shell
        // (the one that ran `coord assign`) would eat the Enter.  Other keys
        // fall through so the shell stays usable if the operator ignores it.
        if self.pending_auto_review.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_auto_review();
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Escape) | Key::Char('n') | Key::Char('N') => {
                        // #722: when the blocking dialog is showing (live session
                        // is still running), Esc must NOT destroy the pending
                        // offer — the operator is expected to reattach and /exit
                        // first, at which point the offer re-fires automatically.
                        if let Some(p) = self.pending_auto_review.as_ref() {
                            let (num, repo) = (p.issue_num, p.coord_repo.clone());
                            if self.offer_pinned_by_live_session(num, &repo, "review") {
                                return Some(Reaction::Redraw);
                            }
                        }
                        self.pending_auto_review = None;
                        self.push_toast(
                            "Review deferred",
                            "Start it any time from the row's right-click menu.",
                            ToastSeverity::Info,
                        );
                        return Some(Reaction::Redraw);
                    }
                    _ => {}
                }
            }
        }

        // ── Post-review one-key stage offer (Fix / Test) confirm ─────────
        // Own Enter (→ launch the next stage) and Esc/n (defer).  No text
        // input, so other keys fall through and the shell stays usable.
        if self.pending_stage_launch.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_stage_launch();
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Escape) | Key::Char('n') | Key::Char('N') => {
                        // #722: preserve the offer when the blocking dialog is
                        // showing — same guard as pending_auto_review above.
                        if let Some(p) = self.pending_stage_launch.as_ref() {
                            let (num, repo) = (p.issue_num, p.coord_repo.clone());
                            if self.offer_pinned_by_live_session(num, &repo, "stage") {
                                return Some(Reaction::Redraw);
                            }
                        }
                        self.pending_stage_launch = None;
                        self.push_toast(
                            "Deferred",
                            "Start the next stage any time from the row's right-click menu.",
                            ToastSeverity::Info,
                        );
                        return Some(Reaction::Redraw);
                    }
                    _ => {}
                }
            }
        }

        // ── Leg 3 (#517 / #587): rework (request-changes) confirm ───────────
        // #587: the rework dialog now owns a findings text input, so ALL key
        // events are consumed here (same discipline as `pending_test_fail`):
        //   Enter  → validate findings non-empty → confirm (saves findings +
        //             launches fix) or toast warning (keeps dialog open).
        //   Escape → cancel and defer.
        //   Backspace → edit the findings buffer.
        //   Char   → append to the findings buffer.
        // The `n`/`N` shortcut is intentionally removed: those characters
        // should type into the findings buffer, not dismiss the dialog.
        if self.pending_rework.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_rework();
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_rework = None;
                        self.push_toast(
                            "Fix deferred",
                            "Start it any time from the row's right-click menu.",
                            ToastSeverity::Info,
                        );
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut p) = self.pending_rework {
                            p.findings.pop();
                        }
                        return Some(Reaction::Redraw);
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut p) = self.pending_rework {
                            p.findings.push(*ch);
                        }
                        return Some(Reaction::Redraw);
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── Leg 3c / A3 (#517, #581): test failed → start fix confirm ────
        // Same intercept discipline: own Enter (→ launch interactive --fix-of
        // briefed with the failure) and Esc/n (dismiss).
        if self.pending_test_fix.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_test_fix();
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Escape) | Key::Char('n') | Key::Char('N') => {
                        // #722: preserve the offer when the blocking dialog is showing.
                        if let Some(p) = self.pending_test_fix.as_ref() {
                            let (num, repo) = (p.issue_num, p.coord_repo.clone());
                            if self.offer_pinned_by_live_session(num, &repo, "fix") {
                                return Some(Reaction::Redraw);
                            }
                        }
                        self.pending_test_fix = None;
                        self.push_toast(
                            "Fix deferred",
                            "Start it any time from the row's right-click menu.",
                            ToastSeverity::Info,
                        );
                        return Some(Reaction::Redraw);
                    }
                    _ => {}
                }
            }
        }

        // ── Leg 3c (#517, #306): test passed → start merge agent confirm ─
        // Own Enter (→ launch interactive --merge-of) and Esc/n (dismiss).
        if self.pending_merge.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_merge();
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Escape) | Key::Char('n') | Key::Char('N') => {
                        // #722: preserve the offer when the blocking dialog is showing.
                        if let Some(p) = self.pending_merge.as_ref() {
                            let (num, repo) = (p.issue_num, p.coord_repo.clone());
                            if self.offer_pinned_by_live_session(num, &repo, "merge") {
                                return Some(Reaction::Redraw);
                            }
                        }
                        self.pending_merge = None;
                        self.push_toast(
                            "Merge deferred",
                            "Start it any time from the row's right-click menu.",
                            ToastSeverity::Info,
                        );
                        return Some(Reaction::Redraw);
                    }
                    _ => {}
                }
            }
        }

        // ── #863: iteration cap reached → force-past-cap confirm ─────────
        // Own Enter (→ re-dispatch the same Fix with --force) and Esc/n (dismiss).
        if self.pending_fix_force_confirm.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_fix_force_past_cap();
                        return Some(Reaction::Redraw);
                    }
                    Key::Named(NamedKey::Escape) | Key::Char('n') | Key::Char('N') => {
                        // #722: preserve the offer when the blocking dialog is showing.
                        if let Some(p) = self.pending_fix_force_confirm.as_ref() {
                            let (num, repo) = (p.issue_num, p.coord_repo.clone());
                            if self.offer_pinned_by_live_session(num, &repo, "force-fix") {
                                return Some(Reaction::Redraw);
                            }
                        }
                        self.pending_fix_force_confirm = None;
                        self.push_toast(
                            "Not forcing",
                            "Resolve manually, or bump pipeline.max_review_iterations \
                             in coordinator.yml.",
                            ToastSeverity::Info,
                        );
                        return Some(Reaction::Redraw);
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// One-key choice modals and the prompts they chain into: the #685
    /// test-mode choice, the #486 fleet-machine picker, the #954 new-terminal
    /// machine picker and its optional-name prompt, the #2147 quiet-hours
    /// window, and the #353 repo picker.
    pub(crate) fn handle_choice_picker_keys(&mut self, event: &UiEvent) -> Option<Reaction> {
        // ── #685: Test-mode choice dialog ─────────────────────────────────────
        // 1/Enter = default (smoke or existing mode), 2 = the other option, Esc = cancel.
        if self.pending_test_mode_choice.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                let chosen: Option<&str> = match key {
                    Key::Named(NamedKey::Enter) => {
                        // Enter confirms the default (pre-selected) option.
                        let is_smoke_default = self
                            .pending_test_mode_choice
                            .as_ref()
                            .map(|p| p.current_mode.as_deref().map(|m| m != "auto").unwrap_or(true))
                            .unwrap_or(true);
                        if is_smoke_default { Some("smoke") } else { Some("auto") }
                    }
                    Key::Char('1') => Some("smoke"),
                    Key::Char('2') => Some("auto"),
                    Key::Named(NamedKey::Escape) => {
                        self.pending_test_mode_choice = None;
                        *self.dialog_layout.borrow_mut() = None;
                        return Some(Reaction::Redraw);
                    }
                    _ => None,
                };
                if let Some(mode) = chosen {
                    if let Some(choice) = self.pending_test_mode_choice.take() {
                        self.confirm_test_mode_choice(choice, mode);
                    }
                    *self.dialog_layout.borrow_mut() = None;
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #486 Leg 4: Pending fleet-machine picker ───────────────────────────
        // Armed for a remote Review/Fix launch when >1 machine can run the repo.
        // Numeric keys (1, 2, …) pick the machine and launch; Esc cancels.
        if self.pending_machine_picker.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char(ch) if ch.is_ascii_digit() && *ch != '0' => {
                        let digit = (*ch as u32 - '1' as u32) as usize;
                        if let Some(picker) = self.pending_machine_picker.as_ref() {
                            if digit < picker.machines.len() {
                                let mode = picker.mode;
                                let machine = picker.machines[digit].name.clone();
                                self.pending_machine_picker = None;
                                self.launch_interactive_session_on_machine(mode, machine, None, false);
                                return Some(Reaction::Redraw);
                            }
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_machine_picker = None;
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #954: pending "new terminal" machine picker ──────────────────────
        // Armed by `open_new_terminal_picker` (`n` in the Terminal view) when
        // >1 fleet machine is configured. Numeric keys (1, 2, …) pick the
        // machine and open the optional-name prompt; Esc cancels.
        if self.pending_new_terminal_picker.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char(ch) if ch.is_ascii_digit() && *ch != '0' => {
                        let digit = (*ch as u32 - '1' as u32) as usize;
                        if let Some(machines) = self.pending_new_terminal_picker.as_ref() {
                            if digit < machines.len() {
                                let machine = machines[digit].name.clone();
                                self.pending_new_terminal_picker = None;
                                self.begin_new_terminal_name_prompt(machine);
                                return Some(Reaction::Redraw);
                            }
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_new_terminal_picker = None;
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #954: pending "new terminal" optional name input ─────────────────
        // Armed by `begin_new_terminal_name_prompt` once a machine is chosen
        // (picker selection, or the single-machine fast path). Enter creates
        // + attaches via `create_and_attach_terminal` (empty buffer ⇒
        // auto-generated slug). Esc cancels.
        if self.pending_new_terminal.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        if let Some(input) = self.pending_new_terminal.take() {
                            self.create_and_attach_terminal(input.machine, input.buf);
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_new_terminal = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut input) = self.pending_new_terminal {
                            input.buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut input) = self.pending_new_terminal {
                            input.buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #2147: pending "Set quiet hours…" window input ────────────────────
        // Armed by the Machines-panel right-click menu
        // (`open_quiet_hours_dialog`). Enter validates + spawns via
        // `submit_quiet_hours` — which re-arms `pending_quiet_hours` (buffer
        // intact) on a parse error rather than closing, so malformed input
        // keeps the dialog open with nothing spawned. Esc cancels outright.
        if self.pending_quiet_hours.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        self.submit_quiet_hours();
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_quiet_hours = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut input) = self.pending_quiet_hours {
                            input.buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut input) = self.pending_quiet_hours {
                            input.buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #353: Pending repo picker for [Add] button ─────────────────────────
        // When multiple repos exist, this shows a numeric picker (1, 2, …).
        // Numeric keys select a repo, Enter dispatches, Esc cancels.
        if self.pending_repo_picker.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char(ch) if ch.is_ascii_digit() && *ch != '0' => {
                        let digit = (*ch as u32 - '1' as u32) as usize;
                        if let Some(ref mut picker) = self.pending_repo_picker {
                            if digit < picker.repos.len() {
                                let repo = picker.repos[digit].clone();
                                self.pending_repo_picker = None;
                                self.dispatch_board_chat_new_issue(&repo);
                                return Some(Reaction::Redraw);
                            }
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_repo_picker = None;
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        None
    }

    /// Single-field text prompts that own ALL input until submit or cancel:
    /// the #200 test-fail reason, #296 report-and-dispatch-fix, #977 fast plan
    /// capture, #1017 new-milestone-via-chat title, and #1003 Plans-row input.
    pub(crate) fn handle_text_prompt_keys(&mut self, event: &UiEvent) -> Option<Reaction> {
        // ── #200 Pending test-fail reason: intercept all keys until submit ────
        // Enter submits and records test_state=failed. Esc cancels. Backspace
        // edits. Any printable char appends.
        if self.pending_test_fail.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        let reason = self
                            .pending_test_fail
                            .as_ref()
                            .map(|(_, b)| b.trim().to_string())
                            .unwrap_or_default();
                        let reason_opt = if reason.is_empty() {
                            None
                        } else {
                            Some(reason.as_str())
                        };
                        self.record_test_verdict("failed", reason_opt);
                        self.pending_test_fail = None;
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_test_fail = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some((_, ref mut buf)) = self.pending_test_fail {
                            buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some((_, ref mut buf)) = self.pending_test_fail {
                            buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #296 Pending "report & dispatch fix" input: intercept all keys ───
        // `r` in Pipeline/Test-gate-actionable opens this buffer.
        // Enter records test_state=failed AND dispatches `coord fix`.
        // Esc cancels without recording anything.
        if self.pending_report_fix.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        let description = self.pending_report_fix.take().unwrap_or_default();
                        let description = description.trim().to_string();
                        let reason_opt = if description.is_empty() {
                            None
                        } else {
                            Some(description.as_str())
                        };
                        // Record the failure verdict first.
                        if self.record_test_verdict("failed", reason_opt) {
                            // Then dispatch a fix worker via `coord fix`.
                            if let Some(work_id) = self.pipeline_selected_work_id() {
                                let args: Vec<String> = if description.is_empty() {
                                    vec!["fix".to_string(), work_id.clone()]
                                } else {
                                    vec![
                                        "fix".to_string(),
                                        work_id.clone(),
                                        "--guidance".to_string(),
                                        description.clone(),
                                    ]
                                };
                                let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                                let issue_num = self
                                    .pipeline_sel
                                    .and_then(|i| self.pipeline_issues.get(i))
                                    .map(|iss| iss.number)
                                    .unwrap_or(0);
                                use crate::commands::SpawnQueuedOutcome;
                                match self.command_runner.spawn_queued(&args_ref) {
                                    SpawnQueuedOutcome::Deduped => {}
                                    SpawnQueuedOutcome::Queued => {
                                        self.push_toast(
                                            "Fix worker queued",
                                            &format!("Fix worker queued for #{} — will dispatch after current command.", issue_num),
                                            ToastSeverity::Info,
                                        );
                                    }
                                    SpawnQueuedOutcome::Started => {
                                        self.push_toast(
                                            "Fix worker dispatched",
                                            &format!("Fix worker dispatched for #{}", issue_num),
                                            ToastSeverity::Info,
                                        );
                                    }
                                }
                            }
                        }
                        self.pending_report_fix = None;
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_report_fix = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut buf) = self.pending_report_fix {
                            buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut buf) = self.pending_report_fix {
                            buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #977 Pending "fast plan capture" title input: intercept all keys ─
        // `c` in the Plans panel opens this buffer. Enter dispatches `coord
        // milestone capture <repo> --title <buf>` via `capture_plan_stub`.
        // Esc cancels without creating anything.
        if self.pending_plan_capture.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        let title = self.pending_plan_capture.take().unwrap_or_default();
                        self.capture_plan_stub(title);
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_plan_capture = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut buf) = self.pending_plan_capture {
                            buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut buf) = self.pending_plan_capture {
                            buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #1017 Pending "New milestone via chat…" title input: intercept
        // all keys ───────────────────────────────────────────────────────
        // Bare `C` in the Plans panel opens this buffer (sibling to #977's
        // `c` capture, above). Enter dispatches `coord milestone chat
        // <repo> --new [--title <buf>]` via `capture_plan_chat` — an empty
        // buffer is a valid submission here (the operator can leave the
        // title for the chat to work out). Esc cancels without dispatching
        // anything.
        if self.pending_new_milestone_chat.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        let title = self.pending_new_milestone_chat.take().unwrap_or_default();
                        self.capture_plan_chat(title);
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_new_milestone_chat = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut buf) = self.pending_new_milestone_chat {
                            buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut buf) = self.pending_new_milestone_chat {
                            buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #1003 Pending Plans-row single-field input: intercept all keys ───
        // Set by "Edit milestone…" / "Add issue to milestone…" / "Remove
        // issue from milestone…" (Plans-panel / MilestoneDag row context
        // menu). Enter submits via `submit_milestone_row_input`. Esc cancels.
        if self.pending_milestone_row_input.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Enter) => {
                        if let Some(input) = self.pending_milestone_row_input.take() {
                            self.submit_milestone_row_input(input);
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.pending_milestone_row_input = None;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some(ref mut input) = self.pending_milestone_row_input {
                            input.buf.pop();
                        }
                    }
                    Key::Char(ch) => {
                        if let Some(ref mut input) = self.pending_milestone_row_input {
                            input.buf.push(*ch);
                        }
                    }
                    _ => {}
                }
                return Some(Reaction::Redraw);
            }
        }

        None
    }

    /// Destructive-action confirmations that intercept every key press:
    /// close/archive plan (#1003), restart, kill terminal (#956), kill session
    /// (#1033), and purge.
    pub(crate) fn handle_destructive_confirm_keys(&mut self, event: &UiEvent) -> Option<Reaction> {
        // ── #1003 Pending "Close / archive plan" confirmation: intercept ALL
        // key presses ─────────────────────────────────────────────────────
        // Set by the Plans-panel / MilestoneDag row context menu's "Close /
        // archive plan" item. 'y'/'Y' confirms (`coord issue close`); every
        // other key cancels. Mirrors `pending_restart`.
        if self.pending_close_plan.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        if let Some(plan) = self.pending_close_plan.take() {
                            self.confirm_close_plan(plan);
                        }
                    }
                    _ => {
                        self.pending_close_plan = None;
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── Pending restart confirmation: intercept ALL key presses ──────────
        // While a restart is pending, 'y'/'Y' fires the restart; every other
        // key cancels.  We return early so normal key dispatch never fires.
        if self.pending_restart.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        if let Some(name) = self.pending_restart.take() {
                            use crate::commands::SpawnQueuedOutcome;
                            if let SpawnQueuedOutcome::Queued = self.command_runner.spawn_queued(&[
                                "agent",
                                "restart",
                                "--machine",
                                &name,
                            ]) {
                                self.push_toast(
                                    "⏳ Queued",
                                    "agent restart runs after current command",
                                    ToastSeverity::Info,
                                );
                            }
                        }
                    }
                    _ => {
                        self.pending_restart = None;
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #956: Pending kill-terminal confirmation: intercept ALL key
        // presses ─────────────────────────────────────────────────────────
        // While a kill is pending, 'y'/'Y' fires it; every other key
        // cancels.  Mirrors `pending_restart` immediately above.
        if self.pending_kill_terminal.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        if let Some(p) = self.pending_kill_terminal.take() {
                            self.confirm_kill_terminal(p);
                        }
                    }
                    _ => {
                        self.pending_kill_terminal = None;
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #1033: Pending kill-session confirmation: intercept ALL key
        // presses ─────────────────────────────────────────────────────────
        // While a kill is pending, 'y'/'Y' fires it; every other key
        // cancels.  Mirrors `pending_kill_terminal` immediately above.
        if self.pending_kill_session.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        if let Some(p) = self.pending_kill_session.take() {
                            self.confirm_kill_session(p);
                        }
                    }
                    _ => {
                        self.pending_kill_session = None;
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── Pending purge confirmation: intercept ALL key presses ─────────────
        // While a purge is pending, 'y'/'Y' executes it; every other key
        // cancels.  We return early so the normal key dispatch never fires.
        if self.pending_purge.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        let secs = self.purge_days as f64 * 86_400.0;
                        match purge_done_assignments_remote(secs) {
                            Ok((a, i)) => self.push_toast(
                                "Purge complete",
                                &format!(
                                    "Removed {} assignment{} + {} closed issue{}",
                                    a,
                                    if a == 1 { "" } else { "s" },
                                    i,
                                    if i == 1 { "" } else { "s" }
                                ),
                                ToastSeverity::Info,
                            ),
                            Err(e) => self.push_toast(
                                "Purge failed",
                                &format!("{}", e),
                                ToastSeverity::Error,
                            ),
                        }
                        self.pending_purge = None;
                        self.refresh();
                    }
                    _ => {
                        // Any other key cancels — Escape, 'n', 'N', or anything else.
                        self.pending_purge = None;
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        None
    }

    /// Keyboard navigation for an open context menu (#259 / #607).
    pub(crate) fn handle_context_menu_keys(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
    ) -> Option<Reaction> {
        // ── #259 / #607: open context menu intercepts keyboard nav ──────────
        // Up/Down/j/k move the keyboard selection (skipping separators);
        // Enter / Right opens a submenu (if selected item has one) or activates;
        // Left / Escape closes the deepest submenu; outer Escape dismisses all.
        if self.pending_context_menu.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Named(NamedKey::Down) | Key::Char('j') => {
                        self.context_menu_move_selection(1);
                    }
                    Key::Named(NamedKey::Up) | Key::Char('k') => {
                        self.context_menu_move_selection(-1);
                    }
                    Key::Named(NamedKey::Enter) => {
                        // Enter: open submenu if parent, else activate leaf.
                        self.context_menu_activate_selected(backend);
                    }
                    Key::Named(NamedKey::Right) => {
                        // Right: open submenu parent only — no-op on leaf items.
                        // This prevents accidental dispatch of Stop/Watch/etc.
                        // when the user arrows past the submenu parents.
                        if self.context_menu_selected_has_submenu() {
                            self.context_menu_activate_selected(backend);
                        }
                    }
                    Key::Named(NamedKey::Left) | Key::Named(NamedKey::Escape) => {
                        // Left / Esc: close deepest submenu or dismiss entirely.
                        self.context_menu_close_submenu_or_dismiss();
                    }
                    _ => {
                        // Any other key dismisses to keep the focus model
                        // simple — typing a global keybind while the menu
                        // is open shouldn't both dismiss and fire that
                        // bind, so we just dismiss.
                        self.dismiss_context_menu();
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        None
    }

    /// Merge-path confirmations: `--force-merge` (#245), revalidate (#2402),
    /// and merge-all-ready (#780).
    pub(crate) fn handle_merge_confirm_keys(&mut self, event: &UiEvent) -> Option<Reaction> {
        // ── #245: Pending --force-merge confirmation: intercept ALL keys ──
        // The user has pressed `m` while the "Checks failed" hint was visible.
        // We refuse to bypass the CI gate without an explicit y/Y so a
        // fat-fingered `m` can't merge a red PR.
        if let Some(repo) = self.pending_force_merge.clone() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        let scoped = !repo.is_empty();
                        let mut args: Vec<&str> = vec!["merge", "--force-merge"];
                        if scoped {
                            args.push("--repo");
                            args.push(&repo);
                        }
                        use crate::commands::SpawnQueuedOutcome;
                        let scope_str = if scoped {
                            format!(" --repo {}", repo)
                        } else {
                            String::new()
                        };
                        match self.command_runner.spawn_queued(&args) {
                            SpawnQueuedOutcome::Started => {
                                self.push_toast(
                                    "Force-merge dispatched",
                                    &format!(
                                        "coord merge --force-merge{} — CI gate bypassed",
                                        scope_str
                                    ),
                                    ToastSeverity::Warning,
                                );
                            }
                            SpawnQueuedOutcome::Queued => {
                                self.push_toast(
                                    "⏳ Queued",
                                    "force-merge runs after current command",
                                    ToastSeverity::Info,
                                );
                            }
                            SpawnQueuedOutcome::Deduped => {}
                        }
                        self.pending_force_merge = None;
                    }
                    _ => {
                        // Any other key cancels — Escape, 'n', 'N', anything.
                        self.pending_force_merge = None;
                        self.push_toast(
                            "Force-merge cancelled",
                            "CI gate stays in place",
                            ToastSeverity::Info,
                        );
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #2402: Pending revalidate confirmation: intercept ALL keys ────
        // Mirrors the force-merge intercept above: `--revalidate` runs a
        // real local build+test (or CI re-run) on the daemon host, so a
        // fat-fingered keypress must not fire it — only an explicit y/Y.
        if let Some(pending) = self.pending_merge_revalidate.clone() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        self.pending_merge_revalidate = None;
                        self.confirm_merge_revalidate(pending);
                    }
                    _ => {
                        // Any other key cancels — Escape, 'n', 'N', anything.
                        self.pending_merge_revalidate = None;
                        self.push_toast(
                            "Revalidate cancelled",
                            "entry stays blocked — nothing changed",
                            ToastSeverity::Info,
                        );
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #780: Merge-all-ready confirm: intercept key presses ──────────
        if let Some(aids) = self.pending_merge_all_ready.clone() {
            if let UiEvent::KeyPressed { key, .. } = event {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        // Drain the entire queue — `coord merge` already merges in
                        // READY order; no extra args needed.
                        let args: Vec<&str> = vec!["merge"];
                        use crate::commands::SpawnQueuedOutcome;
                        match self.command_runner.spawn_queued(&args) {
                            SpawnQueuedOutcome::Started => {
                                self.push_toast(
                                    "Merge all ready dispatched",
                                    &format!("coord merge — {} entr{} queued",
                                        aids.len(),
                                        if aids.len() == 1 { "y" } else { "ies" }),
                                    ToastSeverity::Info,
                                );
                            }
                            SpawnQueuedOutcome::Queued => {
                                self.push_toast(
                                    "⏳ Queued",
                                    "merge runs after current command",
                                    ToastSeverity::Info,
                                );
                            }
                            SpawnQueuedOutcome::Deduped => {}
                        }
                        self.pending_merge_all_ready = None;
                    }
                    _ => {
                        // Any other key cancels.
                        self.pending_merge_all_ready = None;
                        self.push_toast(
                            "Merge all cancelled",
                            "Queue unchanged",
                            ToastSeverity::Info,
                        );
                    }
                }
                return Some(Reaction::Redraw);
            }
        }

        None
    }

    /// Read-only info dialogs, in render priority order: the #816 PTY-panic
    /// dialog, the #1059 Gate-A / #2863 decomposition dispatch-failure dialog,
    /// then the #532 artifact-pull dialog. The first two swallow everything but
    /// Esc/Enter so the wrapped failure reason stays readable; they are checked
    /// before the artifact dialog so that when both are alive the one actually
    /// drawn on top is the one receiving keys.
    pub(crate) fn handle_info_dialog_keys(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
    ) -> Option<Reaction> {
        // ── #816: PTY-panic dialog key intercept ────────────────────────────
        // Esc and Enter dismiss; any other key is swallowed to keep the
        // dialog visible and let the operator read the fault message.
        //
        // This block intentionally runs BEFORE the artifact_pull_dialog
        // intercept below, matching the rendering priority established in
        // build_prompt_dialog (pty_panic_dialog is returned first / shown on
        // top).  When both dialogs are simultaneously active the operator sees
        // the PTY-panic dialog and their keystrokes must be routed to it first.
        if self.pty_panic_dialog.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                let dismiss = matches!(
                    key,
                    Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter)
                );
                if dismiss {
                    self.pty_panic_dialog = None;
                    *self.dialog_layout.borrow_mut() = None;
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #1059: Gate A dispatch-failure dialog key intercept ─────────────
        // Higher priority than the artifact-pull dialog below (mirrors the
        // pty_panic ordering): Esc / Enter dismiss, other keys are swallowed
        // so the full failure reason stays readable.
        // #2863: the decomposition-dispatch failure dialog shares this
        // intercept — same modal shape, same dismiss keys, same reason for
        // swallowing everything else (the full wrapped reason must stay
        // readable rather than vanish on a stray Tab / arrow key).
        if self.gate_a_error_dialog.is_some() || self.decompose_chat_error_dialog.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                let dismiss = matches!(
                    key,
                    Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter)
                );
                if dismiss {
                    self.gate_a_error_dialog = None;
                    self.decompose_chat_error_dialog = None;
                    *self.dialog_layout.borrow_mut() = None;
                }
                return Some(Reaction::Redraw);
            }
        }

        // ── #532: Artifact-pull dialog: intercept key presses ──────────────
        // While the info dialog is open:
        //   'c'/'C' — copy path to clipboard (when available), then dismiss.
        //   Esc / Enter — dismiss without copying.
        //   All other keys — swallow (redraw) without dismissing; this lets
        //   longer error messages be scrolled and prevents accidental dismiss
        //   on Tab / arrow keys while the dialog is focused.
        //
        // This block intentionally runs AFTER the destructive-confirmation
        // intercepts (pending_purge, pending_force_merge, pending_restart) AND
        // after the pty_panic_dialog intercept above, so that if both an
        // artifact dialog and a higher-priority dialog are alive at the same
        // time, the higher-priority one wins — matching the rendering priority
        // in build_prompt_dialog and avoiding silently swallowed keystrokes
        // against a hidden artifact dialog.
        if self.artifact_pull_dialog.is_some() {
            if let UiEvent::KeyPressed { key, .. } = event {
                let path = self
                    .artifact_pull_dialog
                    .as_ref()
                    .and_then(|d| d.path.clone());
                // Classification lives in a pure helper so tests cover the
                // exact key match that production uses.
                match classify_artifact_pull_dialog_key(key, path.is_some()) {
                    ArtifactDialogKeyOutcome::CopyAndDismiss => {
                        if let Some(p) = path {
                            backend.services().clipboard().write_text(&p);
                            self.push_toast(
                                "Copied",
                                "Path copied to clipboard",
                                ToastSeverity::Info,
                            );
                        }
                        self.artifact_pull_dialog = None;
                        *self.dialog_layout.borrow_mut() = None;
                    }
                    ArtifactDialogKeyOutcome::Dismiss => {
                        self.artifact_pull_dialog = None;
                        *self.dialog_layout.borrow_mut() = None;
                    }
                    ArtifactDialogKeyOutcome::Swallow => {
                        // All other keys are swallowed but do NOT close the
                        // dialog — keeps the dialog visible so the user can
                        // read it.  (No scroll offset is tracked here; arrow
                        // keys are simply absorbed.)
                    }
                }
                return Some(Reaction::Redraw);
            }
        }
        None
    }
}
