//! The step history of the open project (`history/steps.json`): which
//! functions of the menu ran, automatically or corrected by hand, as the
//! [`StepRecord`]s of a [`StepHistory`], the same records the CLI and the MCP
//! server read and write (`steps.list`, `project.save`).
//!
//! The history follows the case, version by version ([`Steps`]):
//!
//! * a run of a function of the menu that lands as a new version of the case
//!   is recorded ([`StepHistory::checkpoint`]); a correction made with the
//!   tools of its inspector is recorded as done by hand
//!   ([`StepHistory::mark_manual`]);
//! * a step of the workflow run again or corrected by hand leaves what was
//!   computed from its previous result out of date: the records of the
//!   functions after it go (the steps in the order of the menu, then the
//!   functions that are not steps). The import is the exception: a scan
//!   added changes nothing computed before, and the module clears what was
//!   computed from a scan it replaces;
//! * after every change, a record goes when the case no longer holds the
//!   result of its function ([`dc_module_api::Module::leaf_done`]), and a
//!   failure (another front end may record one) goes once it does. So a
//!   function that leaves nothing in the case, such as an export, counts as
//!   done only on the case its run left, until a change that recomputes it
//!   (a step run again or corrected, the whole pipeline, a setting; not the
//!   run of another such function that only adds its own result, nor the
//!   aligner of the view built or hidden: [`By::Add`], [`By::Look`]). A run is
//!   recorded only when the case holds its result, or when the function
//!   leaves nothing in the case to check;
//! * undo and redo bring back the history of their version;
//! * a history read from a file keeps a record of a step of the workflow only
//!   when the case holds its result (another front end may have changed the
//!   case without its history), and every record of the other functions of
//!   the menu until the first change, as above. `[]` is the empty history
//!   ([`dc_project::Project::history`]); a history that cannot be read is
//!   reported ([`Steps::unreadable`]) and the file stays read-only;
//! * a record of a function this menu does not have (written by a newer
//!   version of the module) is never dropped: nothing here can check it, and
//!   it goes back to the file with the others.
//!
//! A failed run records nothing: the commands of the workspace change the
//! case all or nothing, so after a failure the case and its records are the
//! ones before ([`StepHistory::fail`] drops the records of the steps after
//! the failed one, for a pipeline that loses its partial results).

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use dc_case::CaseState;
use dc_command::{StepDef, StepGraph, StepHistory, StepOutcome, StepRecord};
use dc_module_api::{MenuTree, is_import_scans_command};
use serde_json::{Value, json};

/// What made a new version of the case, for its step history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum By {
    /// A run of the function with this command id.
    Run(&'static str),
    /// A correction by hand of the function with this command id.
    Hand(&'static str),
    /// A run of a function that is not a step and adds its own result to
    /// the case, or leaves a trace of it there (an export, the study
    /// models), without recomputing what the other such functions are made
    /// of: their records stay.
    Add(&'static str),
    /// A change of how the case is shown (the aligner of the view built or
    /// hidden) that leaves every result as it was: no function ran, no
    /// record goes because of it.
    Look,
}

impl By {
    fn function(self) -> Option<&'static str> {
        match self {
            Self::Run(id) | Self::Hand(id) | Self::Add(id) => Some(id),
            Self::Look => None,
        }
    }

    /// `true` when the records of the functions that are not steps stay.
    fn keeps_outputs(self) -> bool {
        matches!(self, Self::Add(_) | Self::Look)
    }
}

/// The step history of each version of the case (see the module docs).
pub(crate) struct Steps {
    /// The functions of the menu (command ids), the steps of the workflow
    /// first, each with `true` when it is a step.
    functions: Vec<(&'static str, bool)>,
    /// The same functions as the pipeline of the records.
    graph: Option<StepGraph>,
    /// The version of the case [`Steps::current`] belongs to.
    of: Weak<CaseState>,
    /// The history of that version.
    current: StepHistory,
    /// The history of every version it described, for undo and redo. A
    /// version the undo stack dropped goes at the next change.
    versions: Vec<(Weak<CaseState>, StepHistory)>,
    /// Why the history of the file could not be read.
    unreadable: Option<String>,
}

impl Steps {
    /// The history `history` (the JSON of `history/steps.json`) of the case
    /// `state` it was saved with, for the functions of `menu`. `shows(id)`:
    /// `state` holds the result of function `id`.
    pub(crate) fn open(
        menu: &MenuTree,
        history: Value,
        state: &Arc<CaseState>,
        shows: impl Fn(&str) -> bool,
    ) -> Self {
        let mut functions: Vec<(&'static str, bool)> = Vec::new();
        for (_, item) in menu.iter() {
            if let Some(id) = item.command_id()
                && !functions.iter().any(|(f, _)| *f == id)
            {
                functions.push((id, item.counts_as_step));
            }
        }
        // the steps first: the other functions work on their results
        functions.sort_by_key(|(_, step)| !step);
        let graph = StepGraph::new(
            functions
                .iter()
                .map(|&(id, _)| StepDef { id, depends: &[] })
                .collect(),
        )
        .ok();
        let (file, unreadable) = read(history);
        let mut steps = Self {
            functions,
            graph,
            of: Weak::new(),
            current: StepHistory::new(),
            versions: Vec::new(),
            unreadable,
        };
        let mut records = file.records().clone();
        records.retain(|id, r| match (r.failed.is_some(), steps.is_step(id)) {
            (true, _) => !shows(id),
            (false, true) => shows(id),
            (false, false) => true,
        });
        let history = rebuilt(file.rev(), records);
        steps.set(state, history);
        steps
    }

    /// The history of the version of the case it follows.
    pub(crate) fn history(&self) -> &StepHistory {
        &self.current
    }

    /// Why the history of the file could not be read (`None`: it could). Its
    /// records are lost from this history: the file must not be written over.
    pub(crate) fn unreadable(&self) -> Option<&str> {
        self.unreadable.as_deref()
    }

    /// A save worked: the file it wrote (never the one whose history could
    /// not be read) holds this history, which can be read.
    pub(crate) fn saved(&mut self) {
        self.unreadable = None;
    }

    /// The function of the menu whose command id is `id`.
    pub(crate) fn function(&self, id: &str) -> Option<&'static str> {
        self.functions
            .iter()
            .find(|(f, _)| *f == id)
            .map(|(f, _)| *f)
    }

    /// The function of the menu that imports scans.
    pub(crate) fn import(&self) -> Option<&'static str> {
        self.functions
            .iter()
            .map(|(f, _)| *f)
            .find(|f| is_import_scans_command(f))
    }

    pub(crate) fn is_step(&self, id: &str) -> bool {
        self.functions.iter().any(|(f, step)| *f == id && *step)
    }

    /// `true` when function `id` is not a step of the workflow and the
    /// history of `state` has a record of a run of it that worked: the
    /// result of such a function may be nowhere in the case (an export).
    pub(crate) fn done_by_record(&self, state: &Arc<CaseState>, id: &str) -> bool {
        self.describes(state)
            && self.functions.iter().any(|(f, step)| *f == id && !step)
            && self.current.record(id).is_some_and(|r| r.failed.is_none())
    }

    fn describes(&self, state: &Arc<CaseState>) -> bool {
        std::ptr::eq(self.of.as_ptr(), Arc::as_ptr(state))
    }

    /// A change of the case landed as the version `state`, made by `by`
    /// (`None`: no run or correction of a function, such as a setting, the
    /// view or a scan removed). `shows(id)`: `state` holds the result of
    /// function `id`.
    pub(crate) fn changed(
        &mut self,
        state: &Arc<CaseState>,
        by: Option<By>,
        shows: impl Fn(&str) -> bool,
    ) {
        let mut records = self.current.records().clone();
        if let Some(f) = by.and_then(By::function)
            && self.is_step(f)
            && !is_import_scans_command(f)
            && let Some(k) = self.functions.iter().position(|(id, _)| *id == f)
        {
            for (id, _) in &self.functions[k + 1..] {
                records.remove(*id);
            }
        }
        // a result the case no longer holds, a failure it now has a result
        // for; a function this menu does not have cannot be checked; what a
        // function that is not a step left stays through a change that does
        // not recompute it (the case holds no trace of it to check)
        let keeps = by.is_some_and(By::keeps_outputs);
        records.retain(|id, r| {
            self.function(id).is_none()
                || r.failed.is_some() != shows(id)
                || (keeps && r.failed.is_none() && !self.is_step(id))
        });
        let mut history = rebuilt(self.current.rev(), records);
        if let Some(g) = &self.graph {
            let recorded = match by {
                Some(By::Run(id) | By::Add(id)) if !self.is_step(id) || shows(id) => {
                    history.checkpoint(g, id, StepOutcome::default())
                }
                Some(By::Hand(id)) if shows(id) => {
                    history.mark_manual(g, id, StepOutcome::default())
                }
                _ => Ok(0),
            };
            if let Err(e) = recorded {
                tracing::warn!(code = "history-record", source = "project", "{e}");
            }
        }
        self.set(state, history);
    }

    /// The case went back or forward to the version `state` (undo, redo):
    /// the history it had comes back. A version it never described (put on
    /// the undo stack around it) keeps the records `state` still shows.
    pub(crate) fn follow(&mut self, state: &Arc<CaseState>, shows: impl Fn(&str) -> bool) {
        if self.describes(state) {
            return;
        }
        let known = self
            .versions
            .iter()
            .find(|(v, _)| std::ptr::eq(v.as_ptr(), Arc::as_ptr(state)))
            .map(|(_, h)| h.clone());
        match known {
            Some(history) => {
                self.of = Arc::downgrade(state);
                self.current = history;
            }
            None => self.changed(state, None, shows),
        }
    }

    fn set(&mut self, state: &Arc<CaseState>, history: StepHistory) {
        self.versions
            .retain(|(v, _)| v.strong_count() > 0 && !std::ptr::eq(v.as_ptr(), Arc::as_ptr(state)));
        self.versions.push((Arc::downgrade(state), history.clone()));
        self.of = Arc::downgrade(state);
        self.current = history;
    }
}

/// The history in `value`: `[]` is the empty one. One that does not decode
/// is empty, with the reason.
fn read(value: Value) -> (StepHistory, Option<String>) {
    if value.as_array().is_some_and(Vec::is_empty) {
        return (StepHistory::new(), None);
    }
    match serde_json::from_value(value) {
        Ok(history) => (history, None),
        Err(e) => (StepHistory::new(), Some(e.to_string())),
    }
}

/// A history of exactly `records`, whose next revision follows `rev`.
/// [`StepHistory`] has no call that drops one record: it is decoded from the
/// JSON of `history/steps.json`, which holds just these two.
fn rebuilt(rev: u64, records: BTreeMap<String, StepRecord>) -> StepHistory {
    serde_json::to_value(records)
        .and_then(|steps| serde_json::from_value(json!({ "rev": rev, "steps": steps })))
        .unwrap_or_else(|e| {
            // its own records always decode: never claim what cannot be checked
            tracing::error!(code = "history-rebuild", source = "project", "{e}");
            StepHistory::new()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::Case;
    use dc_module_api::MenuItem;
    use std::cell::RefCell;

    const IMPORT: &str = "m.case.import-scans";
    const FIND: &str = "m.find";
    const SEPARATE: &str = "m.separate";
    const BUILD: &str = "m.build";
    const AUTO: &str = "m.auto";
    const EXPORT: &str = "m.export";
    const STUDY: &str = "m.study";
    /// A function of another version of the module.
    const NEWER: &str = "m.newer";

    /// A menu like the aligners one: the whole pipeline (not a step) among
    /// the steps, the exports (not steps) at the end.
    fn menu() -> MenuTree {
        let leaf = |id: &'static str| MenuItem::command(id, "k", id);
        MenuTree::new(vec![
            MenuItem::submenu("case", "k", vec![leaf(IMPORT), leaf(AUTO).not_a_step()]),
            MenuItem::submenu("teeth", "k", vec![leaf(FIND), leaf(SEPARATE), leaf(BUILD)]),
            MenuItem::submenu(
                "out",
                "k",
                vec![leaf(EXPORT).not_a_step(), leaf(STUDY).not_a_step()],
            ),
        ])
    }

    fn state(code: &str) -> Arc<CaseState> {
        Arc::new(CaseState::new(Case::new(code)))
    }

    /// The functions whose results the case holds, changed by the tests.
    struct Holds(RefCell<Vec<&'static str>>);

    impl Holds {
        fn new(ids: &[&'static str]) -> Self {
            Self(RefCell::new(ids.to_vec()))
        }
        fn set(&self, ids: &[&'static str]) {
            *self.0.borrow_mut() = ids.to_vec();
        }
        fn shows(&self) -> impl Fn(&str) -> bool + '_ {
            |id| self.0.borrow().contains(&id)
        }
    }

    /// The functions with a record, sorted.
    fn ids(steps: &Steps) -> Vec<&str> {
        steps
            .history()
            .records()
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn sorted<'a>(ids: &[&'a str]) -> Vec<&'a str> {
        let mut v = ids.to_vec();
        v.sort_unstable();
        v
    }

    fn by_hand(steps: &Steps, id: &str) -> bool {
        steps.history().record(id).unwrap().by_hand
    }

    /// The JSON of a history another front end wrote: a run of each function
    /// of `runs` in turn (`Some`: it failed with that message).
    fn written(runs: &[(&'static str, Option<&str>)]) -> Value {
        let g = StepGraph::new(
            [IMPORT, FIND, SEPARATE, BUILD, AUTO, EXPORT, STUDY, NEWER]
                .into_iter()
                .map(|id| StepDef { id, depends: &[] })
                .collect(),
        )
        .unwrap();
        let mut h = StepHistory::new();
        for (id, failure) in runs {
            match failure {
                None => {
                    h.checkpoint(&g, id, StepOutcome::default()).unwrap();
                }
                Some(m) => h.fail(&g, id, *m).unwrap(),
            }
        }
        serde_json::to_value(&h).unwrap()
    }

    #[test]
    fn the_steps_come_first_then_the_other_functions() {
        let s = Steps::open(&menu(), json!([]), &state("a"), |_| false);
        let order: Vec<_> = s.functions.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, [IMPORT, FIND, SEPARATE, BUILD, AUTO, EXPORT, STUDY]);
        assert_eq!(s.import(), Some(IMPORT));
        assert_eq!(s.function(EXPORT), Some(EXPORT));
        assert_eq!(s.function(NEWER), None);
    }

    #[test]
    fn an_empty_history_is_empty_and_another_shape_is_unreadable() {
        let s = Steps::open(&menu(), json!([]), &state("a"), |_| false);
        assert!(s.unreadable().is_none());
        assert!(s.history().records().is_empty());
        let s = Steps::open(
            &menu(),
            json!({ "rev": 0, "steps": {} }),
            &state("a"),
            |_| false,
        );
        assert!(s.unreadable().is_none());
        for bad in [json!("not a history"), json!([{ "step": 1 }]), Value::Null] {
            let mut s = Steps::open(&menu(), bad.clone(), &state("a"), |_| false);
            assert!(s.unreadable().is_some(), "{bad}");
            assert!(s.history().records().is_empty());
            s.saved();
            assert!(s.unreadable().is_none());
        }
    }

    #[test]
    fn an_opened_history_keeps_what_the_case_holds_and_what_nothing_can_check() {
        let case = Holds::new(&[IMPORT, FIND]);
        let file = written(&[
            (IMPORT, None),
            (FIND, None),
            (SEPARATE, None),
            (EXPORT, None),
            (NEWER, None),
        ]);
        let a = state("a");
        let mut s = Steps::open(&menu(), file, &a, case.shows());
        // the colours the case no longer holds go; the export and the
        // function this version does not know stay
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, EXPORT, NEWER]));
        assert!(s.done_by_record(&a, EXPORT));
        assert!(!s.done_by_record(&a, FIND), "a step is done by its result");
        assert!(!s.done_by_record(&a, NEWER));
        assert!(
            !s.done_by_record(&state("b"), EXPORT),
            "only on its version"
        );
        // the revisions go on from the file's
        assert_eq!(s.history().rev(), 5);
        // a change: the export is done on its version only, the record of
        // the function this version does not know stays; a step run again
        // keeps it too
        s.changed(&state("b"), None, case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, NEWER]));
        s.changed(&state("c"), Some(By::Run(IMPORT)), case.shows());
        s.changed(&state("d"), Some(By::Run(FIND)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, NEWER]));
    }

    #[test]
    fn what_a_function_that_is_not_a_step_left_stays_through_a_change_that_does_not_recompute_it() {
        let case = Holds::new(&[IMPORT, FIND]);
        let file = written(&[(IMPORT, None), (FIND, None), (EXPORT, None), (AUTO, None)]);
        let a = state("a");
        let mut s = Steps::open(&menu(), file, &a, case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, EXPORT, AUTO]));
        // the study models added: the export and the pipeline are not made
        // of them
        let b = state("b");
        s.changed(&b, Some(By::Add(STUDY)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, EXPORT, AUTO, STUDY]));
        assert!(s.done_by_record(&b, EXPORT) && s.done_by_record(&b, AUTO));
        // the aligner of the view built: no function ran, nothing goes
        let c = state("c");
        s.changed(&c, Some(By::Look), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, EXPORT, AUTO, STUDY]));
        assert!(s.done_by_record(&c, EXPORT));
        // a change that is nothing of this (a setting), or the whole
        // pipeline, or a step, leaves them out of date
        let d = state("d");
        s.changed(&d, None, case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND]));
        s.changed(&state("e"), Some(By::Add(EXPORT)), case.shows());
        s.changed(&state("f"), Some(By::Run(AUTO)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, AUTO]));
        s.changed(&state("g"), Some(By::Add(EXPORT)), case.shows());
        s.changed(&state("h"), Some(By::Run(FIND)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND]));
    }

    #[test]
    fn a_run_whose_result_the_case_does_not_hold_is_not_recorded() {
        let case = Holds::new(&[IMPORT]);
        let mut s = Steps::open(
            &menu(),
            written(&[(IMPORT, None)]),
            &state("a"),
            case.shows(),
        );
        // the marks ran on the other arch only, say: the case shows no result
        s.changed(&state("b"), Some(By::Run(FIND)), case.shows());
        assert_eq!(ids(&s), [IMPORT]);
        // a scan left provisional by the import: no record of the import
        let mut s = Steps::open(&menu(), json!([]), &state("c"), |_| false);
        s.changed(&state("d"), Some(By::Run(IMPORT)), |_| false);
        assert!(s.history().records().is_empty());
        // a function that leaves nothing in the case is recorded all the same
        s.changed(&state("e"), Some(By::Run(EXPORT)), |_| false);
        assert_eq!(ids(&s), [EXPORT]);
    }

    #[test]
    fn a_failure_stays_until_the_case_has_a_result() {
        let case = Holds::new(&[IMPORT]);
        let file = written(&[(IMPORT, None), (FIND, Some("no scan"))]);
        let mut s = Steps::open(&menu(), file, &state("a"), case.shows());
        assert!(s.history().record(FIND).unwrap().failed.is_some());
        // another change: the failure stays while the case has no result
        s.changed(&state("b"), None, case.shows());
        assert!(s.history().record(FIND).unwrap().failed.is_some());
        // the case has one now (the CLI ran it): the failure goes
        case.set(&[IMPORT, FIND]);
        s.changed(&state("c"), None, case.shows());
        assert!(s.history().record(FIND).is_none());
    }

    #[test]
    fn a_run_is_recorded_and_a_step_run_again_drops_what_came_after_it() {
        let case = Holds::new(&[]);
        let mut s = Steps::open(&menu(), json!([]), &state("0"), case.shows());
        let steps = [IMPORT, FIND, SEPARATE, BUILD];
        for (i, id) in steps.into_iter().enumerate() {
            case.set(&steps[..=i]);
            s.changed(&state(id), Some(By::Run(id)), case.shows());
        }
        assert_eq!(ids(&s), sorted(&steps));
        assert!(!by_hand(&s, FIND));
        let rev = s.history().record(BUILD).unwrap().rev;
        // the export leaves nothing in the case: done on its version only
        let exported = state("exported");
        s.changed(&exported, Some(By::Run(EXPORT)), case.shows());
        assert!(s.done_by_record(&exported, EXPORT));
        assert!(s.history().record(EXPORT).unwrap().rev > rev);
        // the separation again: the 3D teeth (kept, out of date) and the
        // export were computed from the previous colours
        s.changed(&state("again"), Some(By::Run(SEPARATE)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, SEPARATE]));
        assert!(s.history().record(SEPARATE).unwrap().rev > rev);
    }

    #[test]
    fn a_correction_by_hand_is_recorded_and_drops_what_came_after_it() {
        let case = Holds::new(&[IMPORT, FIND, SEPARATE, BUILD]);
        let file = written(&[
            (IMPORT, None),
            (FIND, None),
            (SEPARATE, None),
            (BUILD, None),
        ]);
        let mut s = Steps::open(&menu(), file, &state("a"), case.shows());
        // a tooth mark moved: the marks are by hand; the colours and the
        // teeth (kept, out of date) go from the history
        s.changed(&state("b"), Some(By::Hand(FIND)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND]));
        assert!(by_hand(&s, FIND));
        // the last mark deleted: the case has no marks, so no record
        case.set(&[IMPORT]);
        s.changed(&state("c"), Some(By::Hand(FIND)), case.shows());
        assert_eq!(ids(&s), [IMPORT]);
    }

    #[test]
    fn the_import_drops_only_what_the_case_no_longer_holds() {
        let case = Holds::new(&[IMPORT, FIND, SEPARATE]);
        let file = written(&[(IMPORT, None), (FIND, None), (SEPARATE, None)]);
        let mut s = Steps::open(&menu(), file, &state("a"), case.shows());
        // a bite scan added: the arches keep their results
        s.changed(&state("b"), Some(By::Run(IMPORT)), case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND, SEPARATE]));
        // a scan replaced: the module cleared what was computed from it
        case.set(&[IMPORT]);
        s.changed(&state("c"), Some(By::Run(IMPORT)), case.shows());
        assert_eq!(ids(&s), [IMPORT]);
    }

    #[test]
    fn a_change_drops_the_records_whose_results_the_case_no_longer_holds() {
        let case = Holds::new(&[IMPORT, FIND, SEPARATE]);
        let file = written(&[
            (IMPORT, None),
            (FIND, None),
            (SEPARATE, None),
            (EXPORT, None),
        ]);
        let a = state("a");
        let mut s = Steps::open(&menu(), file, &a, case.shows());
        assert!(s.done_by_record(&a, EXPORT));
        // a scan removed: the module cleared the colours of its arch only
        case.set(&[IMPORT, FIND]);
        let b = state("b");
        s.changed(&b, None, case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND]));
        assert!(!s.done_by_record(&b, EXPORT));
    }

    #[test]
    fn undo_and_redo_bring_back_the_history_of_their_version() {
        let case = Holds::new(&[IMPORT]);
        let a = state("a");
        let mut s = Steps::open(&menu(), written(&[(IMPORT, None)]), &a, case.shows());
        let b = state("b");
        case.set(&[IMPORT, FIND]);
        s.changed(&b, Some(By::Run(FIND)), case.shows());
        let c = state("c");
        s.changed(&c, Some(By::Run(EXPORT)), case.shows());
        let at_c = serde_json::to_value(s.history()).unwrap();
        // undo twice, redo twice: each version has its history
        s.follow(&b, case.shows());
        assert_eq!(ids(&s), sorted(&[IMPORT, FIND]));
        assert!(!s.done_by_record(&b, EXPORT));
        s.follow(&a, case.shows());
        assert_eq!(ids(&s), [IMPORT]);
        s.follow(&b, case.shows());
        s.follow(&c, case.shows());
        assert_eq!(serde_json::to_value(s.history()).unwrap(), at_c);
        assert!(s.done_by_record(&c, EXPORT));
        // a version it never described keeps what that case holds
        s.follow(&state("d"), |id| id == IMPORT);
        assert_eq!(ids(&s), [IMPORT]);
    }

    #[test]
    fn the_versions_the_undo_stack_dropped_go() {
        let mut s = Steps::open(&menu(), json!([]), &state("a"), |_| false);
        for i in 0..10 {
            s.changed(&state(&i.to_string()), None, |_| false);
        }
        // each state above was gone once the next one came
        assert_eq!(s.versions.len(), 1);
    }

    #[test]
    fn a_rebuilt_history_holds_the_same_records_and_revision() {
        let file = written(&[(IMPORT, None), (FIND, Some("x")), (EXPORT, None)]);
        let h: StepHistory = serde_json::from_value(file.clone()).unwrap();
        let again = rebuilt(h.rev(), h.records().clone());
        assert_eq!(serde_json::to_value(&again).unwrap(), file);
    }
}
