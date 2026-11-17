//! The conformance report: every check, with pass, fail or skip, and the first
//! failing case with what is needed to reproduce it.

use std::fmt;

/// One conformance check. Each is reported as one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Check {
    /// No allocation and no free in process, set_param, notes, transport, reset.
    NoAlloc,
    /// No panic from any call, including hostile parameters, transport and state.
    NoPanic,
    /// No NaN or infinity in the output for finite input.
    FiniteOutput,
    /// No denormal numbers in the output.
    NoDenormals,
    /// Output stays under the configured bound.
    BoundedOutput,
    /// A burst of NaN and infinity at the input does not poison the plugin for good.
    HostileInput,
    /// Parameter values stay finite and in range under hostile writes.
    ParamFuzz,
    /// The reported latency is stable, and (opt in) matches an impulse.
    Latency,
    /// The output is silent once the reported tail has passed.
    Tail,
    /// `reset` in the middle of a tail leaves silence.
    Reset,
    /// Parameters and the plugin's chunk survive a save and a load exactly.
    State,
    /// Every supported layout is accepted and processes.
    Layouts,
    /// Locks on the audio path. Not detectable in user space.
    Locks,
}

pub(crate) const CHECK_COUNT: usize = 13;

impl Check {
    pub const ALL: [Check; CHECK_COUNT] = [
        Check::NoAlloc,
        Check::NoPanic,
        Check::FiniteOutput,
        Check::NoDenormals,
        Check::BoundedOutput,
        Check::HostileInput,
        Check::ParamFuzz,
        Check::Latency,
        Check::Tail,
        Check::Reset,
        Check::State,
        Check::Layouts,
        Check::Locks,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Check::NoAlloc => "no_alloc",
            Check::NoPanic => "no_panic",
            Check::FiniteOutput => "finite_output",
            Check::NoDenormals => "no_denormals",
            Check::BoundedOutput => "bounded_output",
            Check::HostileInput => "hostile_input",
            Check::ParamFuzz => "param_fuzz",
            Check::Latency => "latency",
            Check::Tail => "tail",
            Check::Reset => "reset",
            Check::State => "state",
            Check::Layouts => "layouts",
            Check::Locks => "locks",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Check::NoAlloc => "no allocation or free on the audio path",
            Check::NoPanic => "no panic",
            Check::FiniteOutput => "no NaN or infinity in the output",
            Check::NoDenormals => "no denormal numbers in the output",
            Check::BoundedOutput => "output bounded",
            Check::HostileInput => "recovers from NaN and infinity at the input",
            Check::ParamFuzz => "parameters finite and in range under hostile writes",
            Check::Latency => "latency stable and truthful",
            Check::Tail => "tail honoured",
            Check::Reset => "reset clears the tail",
            Check::State => "state round-trips exactly",
            Check::Layouts => "every supported layout works",
            Check::Locks => "no locks on the audio path",
        }
    }

    fn index(self) -> usize {
        Check::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
        }
    }
}

/// The first case that failed a check. Run again with the same seed (and,
/// to go straight to it, [`Options::filter`](crate::Options::filter) set to
/// the case text) to reproduce it.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub case: String,
    /// The master seed of the run.
    pub seed: u64,
    /// The seed of the case's own generator, derived from the master seed and the case text.
    pub case_seed: u64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckResult {
    pub check: Check,
    pub status: Status,
    /// Assertions evaluated for this check.
    pub cases: u64,
    /// Assertions that failed.
    pub failures: u64,
    pub first_failure: Option<Failure>,
    /// Why the check was skipped, or something worth knowing.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub plugin: String,
    pub seed: u64,
    pub checks: Vec<CheckResult>,
    pub rates_run: Vec<f32>,
    /// Rates the plugin refused through `accepts_sample_rate`.
    pub rates_skipped: Vec<f32>,
    /// Test cases (sessions) run.
    pub sessions: u64,
}

impl Report {
    pub fn result(&self, check: Check) -> &CheckResult {
        &self.checks[check.index()]
    }

    pub fn status(&self, check: Check) -> Status {
        self.result(check).status
    }

    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.status != Status::Fail)
    }

    pub fn failed_checks(&self) -> Vec<Check> {
        self.checks.iter().filter(|c| c.status == Status::Fail).map(|c| c.check).collect()
    }

    /// Panic with the full report when any check failed. For use in `#[test]`.
    pub fn assert_pass(&self) {
        if !self.passed() {
            panic!("conformance failed\n{self}");
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "conformance: {} (seed {:#018x}, {} sessions)", self.plugin, self.seed, self.sessions)?;
        for r in &self.checks {
            let failed = if r.failures > 0 { format!(", {} failed", r.failures) } else { String::new() };
            writeln!(
                f,
                "  [{}] {:<15} {} ({} assertions{})",
                r.status.label(),
                r.check.id(),
                r.check.title(),
                r.cases,
                failed
            )?;
            if let Some(n) = &r.note {
                writeln!(f, "         note: {n}")?;
            }
            if let Some(x) = &r.first_failure {
                writeln!(f, "         first failure: {}", x.message)?;
                writeln!(f, "         case: {}", x.case)?;
                writeln!(f, "         reproduce: seed {:#018x} (case seed {:#018x})", x.seed, x.case_seed)?;
            }
        }
        let rates: Vec<String> = self.rates_run.iter().map(|r| format!("{r}")).collect();
        writeln!(f, "  sample rates run: {}", rates.join(", "))?;
        if !self.rates_skipped.is_empty() {
            let rates: Vec<String> = self.rates_skipped.iter().map(|r| format!("{r}")).collect();
            writeln!(f, "  sample rates refused by the plugin (skipped): {}", rates.join(", "))?;
        }
        writeln!(f, "  result: {}", if self.passed() { "PASS" } else { "FAIL" })
    }
}

/// Collects assertions while the suite runs.
pub(crate) struct Recorder {
    cases: [u64; CHECK_COUNT],
    failures: [u64; CHECK_COUNT],
    first: [Option<Failure>; CHECK_COUNT],
    notes: [Option<String>; CHECK_COUNT],
    forced_skip: [bool; CHECK_COUNT],
    hard_skip: [bool; CHECK_COUNT],
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            cases: [0; CHECK_COUNT],
            failures: [0; CHECK_COUNT],
            first: Default::default(),
            notes: Default::default(),
            forced_skip: [false; CHECK_COUNT],
            hard_skip: [false; CHECK_COUNT],
        }
    }

    pub fn pass(&mut self, check: Check) {
        self.cases[check.index()] += 1;
    }

    pub fn fail(&mut self, check: Check, case: &str, seed: u64, case_seed: u64, message: String) {
        let i = check.index();
        self.cases[i] += 1;
        self.failures[i] += 1;
        if self.first[i].is_none() {
            self.first[i] = Some(Failure { case: case.to_string(), seed, case_seed, message });
        }
    }

    pub fn note(&mut self, check: Check, note: String) {
        let slot = &mut self.notes[check.index()];
        match slot {
            Some(existing) => {
                if !existing.contains(&note) {
                    existing.push_str("; ");
                    existing.push_str(&note);
                }
            }
            None => *slot = Some(note),
        }
    }

    /// Left out through `Options::skip`: reported as skipped whatever happened.
    pub fn hard_skip(&mut self, check: Check) {
        self.hard_skip[check.index()] = true;
        self.note(check, "left out through Options::skip".to_string());
    }

    pub fn force_skip(&mut self, check: Check, note: &str) {
        self.forced_skip[check.index()] = true;
        self.note(check, note.to_string());
    }

    pub fn finish(self, plugin: String, seed: u64, rates_run: Vec<f32>, rates_skipped: Vec<f32>, sessions: u64) -> Report {
        let checks = Check::ALL
            .iter()
            .map(|&check| {
                let i = check.index();
                let status = if self.hard_skip[i] {
                    Status::Skip
                } else if self.failures[i] > 0 {
                    Status::Fail
                } else if self.forced_skip[i] || self.cases[i] == 0 {
                    Status::Skip
                } else {
                    Status::Pass
                };
                CheckResult {
                    check,
                    status,
                    cases: self.cases[i],
                    failures: self.failures[i],
                    first_failure: self.first[i].clone(),
                    note: self.notes[i].clone(),
                }
            })
            .collect();
        Report { plugin, seed, checks, rates_run, rates_skipped, sessions }
    }
}
