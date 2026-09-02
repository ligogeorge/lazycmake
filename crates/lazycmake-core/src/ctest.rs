use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cmake::{ensure_parallel_jobs, run_command, BuildCommand, Generator};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Unknown,
    Pass,
    Fail,
    Skip,
}

impl TestStatus {
    pub fn glyph(self, unicode: bool) -> &'static str {
        match self {
            Self::Pass if unicode => "✓",
            Self::Fail if unicode => "✗",
            Self::Skip if unicode => "◌",
            Self::Pass => "+",
            Self::Fail => "x",
            Self::Skip => "o",
            Self::Unknown => "-",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtestCase {
    pub name: String,
    pub status: TestStatus,
}

#[derive(Debug, Clone)]
pub struct CtestDiscovery {
    pub cases: Vec<CtestCase>,
}

impl CtestDiscovery {
    pub fn parse_json(contents: &str) -> Result<Self> {
        let parsed: CtestShowJson = serde_json::from_str(contents).map_err(|e| Error::Parse(e.to_string()))?;
        let mut cases = parsed
            .tests
            .into_iter()
            .map(|t| CtestCase {
                name: t.name,
                status: TestStatus::Unknown,
            })
            .collect::<Vec<_>>();
        cases.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self { cases })
    }

    pub fn parse_text_listing(contents: &str) -> Result<Self> {
        let mut cases = Vec::new();
        for line in contents.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix("Test #") {
                if let Some(rest) = name.split_once(':') {
                    cases.push(CtestCase {
                        name: rest.1.trim().to_string(),
                        status: TestStatus::Unknown,
                    });
                }
            }
        }
        cases.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self { cases })
    }

    pub fn discover(test_dir: &Path, use_json: bool) -> Result<Self> {
        let mut args = vec!["ctest".to_string()];
        if use_json {
            args.push("--show-only=json-v1".into());
        } else {
            args.push("-N".into());
        }

        let output = std::process::Command::new(&args[0])
            .args(&args[1..])
            .current_dir(test_dir)
            .output()
            .map_err(|e| Error::Cmake(e.to_string()))?;

        if !output.status.success() {
            return Err(Error::Cmake(String::from_utf8_lossy(&output.stderr).into()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if use_json {
            Self::parse_json(&stdout)
        } else {
            Self::parse_text_listing(&stdout)
        }
    }

    pub fn apply_run_output(&mut self, output: &str) {
        let (statuses, final_summary) = parse_run_output(self, output);

        // Only a run covering the whole suite may drop earlier results: a filtered
        // run (single test) says nothing about the cases it never executed.
        if final_summary.is_some_and(|summary| summary.total == self.cases.len()) {
            for case in &mut self.cases {
                case.status = TestStatus::Unknown;
            }
        }

        for (name, status) in statuses {
            set_status(self, &name, status);
        }

        if let Some(summary) = final_summary {
            apply_final_summary(self, summary);
        }
    }

    /// Carry statuses of same-named cases over a rediscovery of the same suite.
    pub fn carry_over_statuses(&mut self, previous: &Self) {
        for case in &mut self.cases {
            if let Some(prev) = previous.cases.iter().find(|prev| prev.name == case.name) {
                case.status = prev.status;
            }
        }
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut pass = 0;
        let mut fail = 0;
        let mut skip = 0;
        let mut unknown = 0;
        for case in &self.cases {
            match case.status {
                TestStatus::Pass => pass += 1,
                TestStatus::Fail => fail += 1,
                TestStatus::Skip => skip += 1,
                TestStatus::Unknown => unknown += 1,
            }
        }
        (pass, fail, skip, unknown)
    }
}

/// Statuses named by a ctest run, plus its final summary line if it printed one.
fn parse_run_output(
    discovery: &CtestDiscovery,
    output: &str,
) -> (Vec<(String, TestStatus)>, Option<FinalSummary>) {
    let mut statuses = Vec::new();
    let mut section = SummarySection::None;
    let mut final_summary = None;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with("The following tests passed") {
            section = SummarySection::Passed;
            continue;
        }
        if line.starts_with("The following tests FAILED") {
            section = SummarySection::Failed;
            continue;
        }

        if let Some(summary) = parse_final_summary_line(line) {
            final_summary = Some(summary);
        }

        if let Some((name, status)) = parse_progress_line(line) {
            statuses.push((name, status));
            continue;
        }

        if let Some((name, status)) = parse_numbered_summary_line(line) {
            statuses.push((name, status));
            continue;
        }

        if matches!(section, SummarySection::Passed) && is_case_name(discovery, line) {
            statuses.push((line.to_string(), TestStatus::Pass));
        } else if matches!(section, SummarySection::Failed) && is_case_name(discovery, line) {
            statuses.push((line.to_string(), TestStatus::Fail));
        }
    }

    (statuses, final_summary)
}

fn set_status(discovery: &mut CtestDiscovery, name: &str, status: TestStatus) {
    if let Some(case) = discovery.cases.iter_mut().find(|c| c.name == name) {
        case.status = status;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FinalSummary {
    failed: usize,
    total: usize,
}

/// `100% tests passed, 0 tests failed out of 314`
fn parse_final_summary_line(line: &str) -> Option<FinalSummary> {
    let failed_idx = line.find(" tests failed out of ")?;
    let before = &line[..failed_idx];
    let failed = before.rsplit_once(',')?.1.trim().parse().ok()?;
    let total = line[failed_idx + " tests failed out of ".len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(FinalSummary { failed, total })
}

/// Whether captured ctest/cmake output indicates a failed test run.
///
/// Important: success summaries contain the substring `0 tests failed`, so a naive
/// `contains("tests failed")` check is wrong.
pub fn ctest_output_indicates_failure(output: &str) -> bool {
    if output.contains("Errors while running CTest") {
        return true;
    }
    let mut summary_failed = None;
    let mut saw_failed_progress = false;
    for line in output.lines() {
        if line.contains("***Failed") {
            saw_failed_progress = true;
        }
        if let Some(summary) = parse_final_summary_line(line) {
            summary_failed = Some(summary.failed);
        }
    }
    if let Some(failed) = summary_failed {
        return failed > 0;
    }
    saw_failed_progress
}

fn apply_final_summary(discovery: &mut CtestDiscovery, summary: FinalSummary) {
    if summary.total != discovery.cases.len() {
        return;
    }
    let (_, fail, _, unknown) = discovery.counts();
    if unknown == 0 {
        return;
    }
    // Failures are listed explicitly at the end; anything still unknown passed (or was skipped
    // without a line we saw). When ctest reports zero failures, treat unknowns as pass.
    if summary.failed == 0 || summary.failed == fail {
        for case in &mut discovery.cases {
            if case.status == TestStatus::Unknown {
                case.status = TestStatus::Pass;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummarySection {
    None,
    Passed,
    Failed,
}

fn is_case_name(discovery: &CtestDiscovery, line: &str) -> bool {
    discovery.cases.iter().any(|c| c.name == line)
}

/// `1/4 Test #167: Name ...` / `2/4 Test  #12: Name ...` / `3/4 Test   #2: Name ...`
fn parse_progress_line(line: &str) -> Option<(String, TestStatus)> {
    let test_idx = line.find("Test")?;
    let after_test = line[test_idx + 4..].trim_start();
    let after_hash = after_test.strip_prefix('#')?;
    let (_num, after_colon) = after_hash.split_once(':')?;
    let name_part = after_colon.trim_start();
    // Name is followed by spaces and a run of dots before the status word.
    let name = name_part
        .split(" ..")
        .next()
        .map(str::trim)
        .filter(|n| !n.is_empty())?;

    let status = if line.contains("***Failed") {
        TestStatus::Fail
    } else if line.contains("***Not Run") || line.contains("***Skipped") {
        TestStatus::Skip
    } else if line.contains(" Passed") || line.contains("\tPassed") {
        TestStatus::Pass
    } else {
        return None;
    };

    Some((name.to_string(), status))
}

/// `1 - AccelerometerStationaryDetector (Failed)`
fn parse_numbered_summary_line(line: &str) -> Option<(String, TestStatus)> {
    let rest = line.find(" - ")? + 3;
    let tail = line.get(rest..)?.trim();
    let name = tail.split(" (").next()?.trim();
    if name.is_empty() {
        return None;
    }

    let status = if tail.contains("(Failed)") {
        TestStatus::Fail
    } else if tail.contains("(Not Run)") || tail.contains("(Skipped)") {
        TestStatus::Skip
    } else if tail.contains("(Passed)") {
        TestStatus::Pass
    } else {
        return None;
    };

    Some((name.to_string(), status))
}

#[derive(Debug, Deserialize)]
struct CtestShowJson {
    #[serde(default)]
    tests: Vec<CtestShowEntry>,
}

#[derive(Debug, Deserialize)]
struct CtestShowEntry {
    name: String,
}

pub struct CtestRunCommand {
    pub test_dir: std::path::PathBuf,
    pub filter: Option<String>,
    pub extra_args: Vec<String>,
}

impl CtestRunCommand {
    pub fn argv(&self) -> Vec<String> {
        self.argv_with_test_dir(true)
    }

    /// CTest args when the process working directory is already `test_dir`.
    pub fn argv_local(&self) -> Vec<String> {
        self.argv_with_test_dir(false)
    }

    fn argv_with_test_dir(&self, use_test_dir_flag: bool) -> Vec<String> {
        let mut args = vec!["ctest".into()];
        if use_test_dir_flag {
            args.push("--test-dir".into());
            args.push(self.test_dir.display().to_string());
        }
        args.extend(self.extra_args.clone());
        if let Some(filter) = &self.filter {
            args.push("-R".into());
            args.push(format!("^{filter}$"));
        }
        ensure_parallel_jobs(&mut args);
        args
    }

    pub fn display_line(&self) -> String {
        self.argv().join(" ")
    }

    pub fn spawn(&self, cwd: &Path) -> Result<std::process::Child> {
        run_command(&self.argv(), cwd)
    }
}

/// One process invocation (args + working directory) for a multi-step job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStep {
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Extra environment variables for this step only (empty for most jobs).
    pub env: Vec<(String, String)>,
}

impl CommandStep {
    pub fn new(args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            args,
            cwd,
            env: Vec::new(),
        }
    }

    pub fn with_env(mut self, env: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env = env.into_iter().collect();
        self.env.sort_by(|a, b| a.0.cmp(&b.0));
        self
    }
}

/// Build every test binary with cmake, then run ctest — same order as `mise run test`.
pub fn test_all_steps(
    testing_binary_dir: &Path,
    test_dir: &Path,
    project_root: &Path,
    generator: Generator,
    extra_args: Vec<String>,
) -> Vec<CommandStep> {
    let build = BuildCommand {
        binary_dir: testing_binary_dir.to_path_buf(),
        target: None,
        clean_first: false,
        generator,
        config: Some("Debug".into()),
    }
    .argv();

    vec![
        CommandStep::new(build, project_root.to_path_buf()),
        CommandStep::new(
            CtestRunCommand {
                test_dir: test_dir.to_path_buf(),
                filter: None,
                extra_args,
            }
            .argv_local(),
            test_dir.to_path_buf(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_fixture() {
        let json = include_str!("../tests/fixtures/ctest-show.json");
        let discovery = CtestDiscovery::parse_json(json).unwrap();
        assert_eq!(discovery.cases.len(), 2);
        assert_eq!(discovery.cases[0].name, "test_a");
    }

    #[test]
    fn apply_run_output_parses_padded_test_numbers() {
        // ctest pads: "Test #100", "Test  #12", "Test   #2"
        let mut discovery = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "ActionQueueTest".into(),
                    status: TestStatus::Unknown,
                },
                CtestCase {
                    name: "Adder16Test".into(),
                    status: TestStatus::Unknown,
                },
                CtestCase {
                    name: "PowerActionQueueTest".into(),
                    status: TestStatus::Unknown,
                },
            ],
        };

        let output = "\
1/3 Test #167: PowerActionQueueTest .............   Passed    0.02 sec
2/3 Test  #12: Adder16Test ......................   Passed    0.02 sec
3/3 Test   #2: ActionQueueTest ..................   Passed    0.02 sec

100% tests passed, 0 tests failed out of 3
";
        discovery.apply_run_output(output);

        assert_eq!(discovery.counts(), (3, 0, 0, 0), "{:?}", discovery.cases);
    }

    #[test]
    fn apply_run_output_summary_marks_remaining_passes() {
        let mut discovery = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "SeenInLog".into(),
                    status: TestStatus::Unknown,
                },
                CtestCase {
                    name: "MissingFromTruncatedLog".into(),
                    status: TestStatus::Unknown,
                },
            ],
        };
        discovery.apply_run_output(
            "1/2 Test #100: SeenInLog ................   Passed    0.01 sec\n\
             100% tests passed, 0 tests failed out of 2\n",
        );
        assert_eq!(discovery.counts(), (2, 0, 0, 0));
    }

    #[test]
    fn apply_run_output_parses_ctest_lines() {
        let mut discovery = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "AccelerometerStationaryDetector".into(),
                    status: TestStatus::Unknown,
                },
                CtestCase {
                    name: "ActivityClassificationPipelineTest".into(),
                    status: TestStatus::Unknown,
                },
            ],
        };

        let output = "\
1/1 Test #1: AccelerometerStationaryDetector ...   Passed    1.73 sec
The following tests passed:
\tAccelerometerStationaryDetector
1/1 Test #284: ActivityClassificationPipelineTest ...***Failed    0.00 sec
The following tests FAILED:
\t  284 - ActivityClassificationPipelineTest (Failed)
Errors while running CTest";
        discovery.apply_run_output(output);

        assert_eq!(discovery.cases[0].status, TestStatus::Pass);
        assert_eq!(discovery.cases[1].status, TestStatus::Fail);
        assert_eq!(discovery.counts(), (1, 1, 0, 0));
    }

    #[test]
    fn counts_tallies_each_status() {
        let discovery = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "a".into(),
                    status: TestStatus::Pass,
                },
                CtestCase {
                    name: "b".into(),
                    status: TestStatus::Pass,
                },
                CtestCase {
                    name: "c".into(),
                    status: TestStatus::Fail,
                },
                CtestCase {
                    name: "d".into(),
                    status: TestStatus::Skip,
                },
                CtestCase {
                    name: "e".into(),
                    status: TestStatus::Unknown,
                },
            ],
        };
        assert_eq!(discovery.counts(), (2, 1, 1, 1));
    }

    #[test]
    fn test_all_builds_then_ctests() {
        let jobs = crate::cmake::max_job_count().to_string();
        let steps = test_all_steps(
            Path::new("build-test"),
            Path::new("build-test/src/tests"),
            Path::new("/proj"),
            Generator::Ninja,
            vec!["--output-on-failure".into(), "--parallel".into()],
        );
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].args,
            vec![
                "cmake",
                "--build",
                "build-test",
                "--parallel",
                &jobs,
            ]
        );
        assert_eq!(steps[0].cwd, PathBuf::from("/proj"));
        assert_eq!(
            steps[1].args,
            vec![
                "ctest",
                "--output-on-failure",
                "--parallel",
                &jobs,
            ]
        );
        assert_eq!(steps[1].cwd, PathBuf::from("build-test/src/tests"));
    }

    #[test]
    fn run_command_anchors_filter() {
        let jobs = crate::cmake::max_job_count().to_string();
        let cmd = CtestRunCommand {
            test_dir: Path::new("build-test/src/tests").into(),
            filter: Some("test_decoder".into()),
            extra_args: vec!["--output-on-failure".into()],
        };
        let args = cmd.argv();
        assert!(args.contains(&"--test-dir".to_string()));
        assert!(args.contains(&"-R".to_string()));
        assert!(args.contains(&"^test_decoder$".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "--parallel" && w[1] == jobs));

        let local = cmd.argv_local();
        assert!(!local.iter().any(|a| a == "--test-dir"));
        assert!(local.contains(&"-R".to_string()));
        assert!(local.windows(2).any(|w| w[0] == "--parallel" && w[1] == jobs));
    }

    #[test]
    fn parses_text_listing() {
        let listing = "\
Test project /tmp/build
  Test #1: zebra_test
  Test #2: alpha_test
Total Tests: 2
";
        let discovery = CtestDiscovery::parse_text_listing(listing).unwrap();
        assert_eq!(discovery.cases.len(), 2);
        assert_eq!(discovery.cases[0].name, "alpha_test");
        assert_eq!(discovery.cases[1].name, "zebra_test");
    }

    #[test]
    fn glyphs_for_unicode_and_ascii() {
        assert_eq!(TestStatus::Pass.glyph(true), "✓");
        assert_eq!(TestStatus::Fail.glyph(true), "✗");
        assert_eq!(TestStatus::Skip.glyph(true), "◌");
        assert_eq!(TestStatus::Unknown.glyph(true), "-");
        assert_eq!(TestStatus::Pass.glyph(false), "+");
        assert_eq!(TestStatus::Fail.glyph(false), "x");
        assert_eq!(TestStatus::Skip.glyph(false), "o");
        assert_eq!(TestStatus::Unknown.glyph(false), "-");
    }

    #[test]
    fn ctest_output_indicates_failure_ignores_zero_failed_summary() {
        let passed = "\
1/1 Test #1: FooTest ................   Passed    0.01 sec

100% tests passed, 0 tests failed out of 1
";
        assert!(!ctest_output_indicates_failure(passed));

        let failed = "\
1/1 Test #1: FooTest ................***Failed    0.01 sec

0% tests passed, 1 tests failed out of 1
";
        assert!(ctest_output_indicates_failure(failed));

        assert!(ctest_output_indicates_failure(
            "Errors while running CTest\n"
        ));
    }

    #[test]
    fn ctest_output_indicates_failure_ignores_stale_failed_progress_when_summary_passes() {
        let accumulated = "\
1/1 Test #1: BarkDetectionTest ................***Failed    0.26 sec

0% tests passed, 1 tests failed out of 1
$ (cd build && ctest --output-on-failure -R BarkDetectionTest)
1/1 Test #1: BarkDetectionTest ................   Passed    0.26 sec

100% tests passed, 0 tests failed out of 1
";
        assert!(!ctest_output_indicates_failure(
            accumulated.split("$ (cd build").nth(1).unwrap_or("")
        ));
    }

    #[test]
    fn carry_over_statuses_keeps_results_of_rediscovered_cases() {
        let previous = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "AlphaTest".into(),
                    status: TestStatus::Pass,
                },
                CtestCase {
                    name: "BetaTest".into(),
                    status: TestStatus::Fail,
                },
            ],
        };
        let mut rediscovered = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "AlphaTest".into(),
                    status: TestStatus::Unknown,
                },
                CtestCase {
                    name: "BetaTest".into(),
                    status: TestStatus::Unknown,
                },
                CtestCase {
                    name: "NewTest".into(),
                    status: TestStatus::Unknown,
                },
            ],
        };

        rediscovered.carry_over_statuses(&previous);

        assert_eq!(rediscovered.cases[0].status, TestStatus::Pass);
        assert_eq!(rediscovered.cases[1].status, TestStatus::Fail);
        assert_eq!(rediscovered.cases[2].status, TestStatus::Unknown);
    }

    #[test]
    fn apply_run_output_of_single_test_keeps_other_statuses() {
        let mut discovery = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "AlphaTest".into(),
                    status: TestStatus::Pass,
                },
                CtestCase {
                    name: "BetaTest".into(),
                    status: TestStatus::Fail,
                },
            ],
        };

        discovery.apply_run_output(
            "1/1 Test #1: BetaTest ...........   Passed    0.02 sec\n\
             100% tests passed, 0 tests failed out of 1\n",
        );

        assert_eq!(discovery.cases[0].status, TestStatus::Pass);
        assert_eq!(discovery.cases[1].status, TestStatus::Pass);
    }

    #[test]
    fn apply_run_output_without_parsable_lines_keeps_statuses() {
        let mut discovery = CtestDiscovery {
            cases: vec![CtestCase {
                name: "AlphaTest".into(),
                status: TestStatus::Pass,
            }],
        };

        discovery.apply_run_output("");

        assert_eq!(discovery.cases[0].status, TestStatus::Pass);
    }

    #[test]
    fn apply_run_output_parses_skipped_and_resets_previous() {
        let mut discovery = CtestDiscovery {
            cases: vec![
                CtestCase {
                    name: "slow_test".into(),
                    status: TestStatus::Pass,
                },
                CtestCase {
                    name: "flaky_test".into(),
                    status: TestStatus::Fail,
                },
            ],
        };
        discovery.apply_run_output(
            "1/1 Test #3: slow_test ...***Skipped    0.00 sec\n\
             9 - flaky_test (Not Run)\n",
        );
        assert_eq!(discovery.cases[0].status, TestStatus::Skip);
        assert_eq!(discovery.cases[1].status, TestStatus::Skip);
        assert_eq!(discovery.counts(), (0, 0, 2, 0));
    }
}
