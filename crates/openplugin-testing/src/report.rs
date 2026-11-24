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
    /// Every check, in the order a [`Report`] lists them.
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

    /// The check's stable machine name, such as `"no_alloc"`: what the report prints and what
    /// scripts can match on.
    ///
    /// ```
    /// use openplugin_testing::Check;
    ///
    /// assert_eq!(Check::NoAlloc.id(), "no_alloc");
    /// assert_eq!(Check::ALL.len(), 13);
    /// ```
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

    /// A one-line English description of what the check asserts, for the report.
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

/// The outcome of one [`Check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// Every assertion of the check held, and at least one ran.
    Pass,
    /// At least one assertion failed; [`CheckResult::first_failure`] says which.
    Fail,
    /// The check did not run: left out through [`Options::skip`](crate::Options::skip), not
    /// applicable to the plugin, not detectable ([`Check::Locks`]), or no case ran. The
    /// note says why.
    Skip,
}

impl Status {
    /// `"pass"`, `"FAIL"` or `"skip"`, as the report prints it.
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
    /// The case's label: rate, block size, stimulus and step, as the suite names it.
    pub case: String,
    /// The master seed of the run.
    pub seed: u64,
    /// The seed of the case's own generator, derived from the master seed and the case text.
    pub case_seed: u64,
    /// What went wrong, in English.
    pub message: String,
}

/// One line of a [`Report`]: a check, its outcome, and its counts.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckResult {
    /// The check this line reports.
    pub check: Check,
    /// Pass, fail or skip.
    pub status: Status,
    /// Assertions evaluated for this check.
    pub cases: u64,
    /// Assertions that failed.
    pub failures: u64,
    /// The first failing case, with its seeds; `None` when nothing failed.
    pub first_failure: Option<Failure>,
    /// Why the check was skipped, or something worth knowing.
    pub note: Option<String>,
}

/// What [`conformance`](crate::conformance) found: one [`CheckResult`] per [`Check`], in the
/// order of [`Check::ALL`]. Its `Display` is the multi-line text report.
///
/// ```
/// # use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
/// # static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// # static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// # #[derive(Default)]
/// # struct MyPlugin { db: f32 }
/// # impl Plugin for MyPlugin {
/// #     fn info(&self) -> &'static PluginInfo { &INFO }
/// #     fn prepare(&mut self, _: f32, _: usize, _: usize) {}
/// #     fn reset(&mut self) {}
/// #     fn set_param(&mut self, id: &str, v: f32) -> bool { if id != "gain" { return false; } self.db = PARAMS[0].clamp(v); true }
/// #     fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
/// #     fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
/// #         let g = 10f32.powf(self.db / 20.0);
/// #         for c in io { for s in &mut c[..frames] { *s *= g; } }
/// #     }
/// # }
/// use openplugin_testing::{conformance, Check, Options, Status};
///
/// let options = Options { sample_rates: vec![48_000.0], block_sizes: vec![64], ..Options::quick() };
/// let report = conformance(MyPlugin::default, options);
/// assert!(report.passed(), "{report}");
/// assert_eq!(report.status(Check::Locks), Status::Skip); // never detectable
/// println!("{report}");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// The plugin's name ([`PluginInfo::name`](openplugin_core::PluginInfo::name)).
    pub plugin: String,
    /// The master seed of the run ([`Options::seed`](crate::Options::seed)).
    pub seed: u64,
    /// One result per check, in the order of [`Check::ALL`].
    pub checks: Vec<CheckResult>,
    /// Sample rates, in Hz, the plugin accepted and was run at.
    pub rates_run: Vec<f32>,
    /// Rates the plugin refused through `accepts_sample_rate`.
    pub rates_skipped: Vec<f32>,
    /// Test cases (sessions) run.
    pub sessions: u64,
}

impl Report {
    /// The result of `check`.
    pub fn result(&self, check: Check) -> &CheckResult {
        &self.checks[check.index()]
    }

    /// The status of `check`: `report.result(check).status`.
    pub fn status(&self, check: Check) -> Status {
        self.result(check).status
    }

    /// True when no check failed. Skipped checks do not fail a run.
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.status != Status::Fail)
    }

    /// The checks that failed, in report order (empty when [`Report::passed`]). Allocates.
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
