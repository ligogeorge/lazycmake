use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::PresetOverrideConfig;
use crate::error::{Error, Result};
use crate::presets::{expand_string, ResolvedConfigurePreset};
use crate::cmake_cache::effective_configure_cache_variables;

/// Logical CPU count for `--parallel` / `-j` (at least 1).
pub fn max_job_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Ensure `args` requests full-core parallelism via `--parallel <n>`.
///
/// Leaves an explicit job count alone (`--parallel 4`, `-j8`). Bare `--parallel` /
/// `-j` get the host core count inserted. Missing flags are appended.
pub fn ensure_parallel_jobs(args: &mut Vec<String>) {
    let jobs = max_job_count().to_string();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--parallel" || arg == "-j" {
            let needs_value = match args.get(i + 1) {
                None => true,
                Some(next) => next.parse::<usize>().is_err(),
            };
            if needs_value {
                args.insert(i + 1, jobs);
            }
            return;
        }
        if let Some(rest) = arg.strip_prefix("-j") {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return;
            }
        }
        i += 1;
    }
    args.push("--parallel".into());
    args.push(jobs);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    Ninja,
    NinjaMultiConfig,
    UnixMakefiles,
    NMakeMakefiles,
    VisualStudio,
    Xcode,
    Other,
}

impl Generator {
    pub fn parse(name: &str) -> Self {
        match name {
            "Ninja" => Self::Ninja,
            "Ninja Multi-Config" => Self::NinjaMultiConfig,
            "Unix Makefiles" => Self::UnixMakefiles,
            "NMake Makefiles" => Self::NMakeMakefiles,
            s if s.contains("Visual Studio") => Self::VisualStudio,
            "Xcode" => Self::Xcode,
            _ => Self::Other,
        }
    }

    pub fn needs_config_flag(&self) -> bool {
        matches!(self, Self::NinjaMultiConfig | Self::VisualStudio | Self::Xcode)
    }

    pub fn exe_suffix(&self) -> &'static str {
        if cfg!(windows) {
            ".exe"
        } else {
            ""
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub presets: bool,
    pub preset_include: bool,
    pub file_api: bool,
    pub ctest_json: bool,
    pub version: String,
}

impl Capabilities {
    pub fn detect() -> Self {
        let output = Command::new("cmake").arg("--version").output();
        let Ok(output) = output else {
            return Self::default();
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let version = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(2))
            .unwrap_or("0.0.0")
            .to_string();

        let major_minor = parse_version(&version);
        Self {
            presets: major_minor >= (3, 19),
            preset_include: major_minor >= (3, 23),
            file_api: major_minor >= (3, 14),
            ctest_json: major_minor >= (3, 14),
            version,
        }
    }
}

fn parse_version(version: &str) -> (u32, u32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor)
}

#[derive(Debug, Clone)]
pub enum ConfigureCommand {
    Preset { name: String },
    Manual {
        source_dir: PathBuf,
        binary_dir: PathBuf,
        generator: Option<String>,
        cache_variables: Vec<(String, String)>,
    },
}

impl ConfigureCommand {
    pub fn from_preset(preset: &ResolvedConfigurePreset) -> Self {
        Self::Preset {
            name: preset.name.clone(),
        }
    }

    /// Build the configure argv for a preset, applying `[presets.overrides.<name>]`
    /// when present. An override with `source_dir` becomes a Manual `-S/-B` invoke
    /// (Zephyr sysbuild). Override-only unhide (empty override body) keeps `--preset`.
    pub fn for_preset(
        preset: &ResolvedConfigurePreset,
        binary_dir: &Path,
        override_cfg: Option<&PresetOverrideConfig>,
        project_root: &Path,
    ) -> Result<Self> {
        let Some(ov) = override_cfg else {
            return Ok(Self::from_preset(preset));
        };

        if ov.source_dir.is_none() && ov.generator.is_none() && ov.cache_variables.is_empty() {
            // Present only to reveal a hidden preset.
            return Ok(Self::from_preset(preset));
        }

        let source_dir = match &ov.source_dir {
            Some(raw) => PathBuf::from(expand_string(raw, project_root, &preset.name)?),
            None => project_root.to_path_buf(),
        };

        let generator = ov
            .generator
            .clone()
            .or_else(|| preset.generator.clone());

        let vars = effective_configure_cache_variables(preset, Some(ov), project_root)?;
        let mut cache_variables: Vec<(String, String)> = vars.into_iter().collect();
        cache_variables.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(Self::Manual {
            source_dir,
            binary_dir: binary_dir.to_path_buf(),
            generator,
            cache_variables,
        })
    }

    pub fn argv(&self, project_root: &Path) -> Vec<String> {
        match self {
            Self::Preset { name } => vec![
                "cmake".into(),
                "--preset".into(),
                name.clone(),
            ],
            Self::Manual {
                source_dir,
                binary_dir,
                generator,
                cache_variables,
            } => {
                let mut args = vec![
                    "cmake".into(),
                    "-S".into(),
                    source_dir.display().to_string(),
                    "-B".into(),
                    binary_dir.display().to_string(),
                ];
                if let Some(g) = generator {
                    args.push("-G".into());
                    args.push(g.clone());
                }
                for (k, v) in cache_variables {
                    args.push(format!("-D{k}={v}"));
                }
                let _ = project_root;
                args
            }
        }
    }

    pub fn display_line(&self, project_root: &Path) -> String {
        self.argv(project_root).join(" ")
    }
}

#[derive(Debug, Clone)]
pub struct BuildCommand {
    pub binary_dir: PathBuf,
    pub target: Option<String>,
    pub clean_first: bool,
    pub generator: Generator,
    pub config: Option<String>,
}

impl BuildCommand {
    pub fn argv(&self) -> Vec<String> {
        let mut args = vec!["cmake".into(), "--build".into(), self.binary_dir.display().to_string()];
        if self.clean_first {
            args.push("--target".into());
            args.push("clean".into());
        }
        if let Some(target) = &self.target {
            args.push("--target".into());
            args.push(target.clone());
        }
        if self.generator.needs_config_flag() {
            if let Some(config) = &self.config {
                args.push("--config".into());
                args.push(config.clone());
            }
        }
        ensure_parallel_jobs(&mut args);
        args
    }

    pub fn display_line(&self) -> String {
        self.argv().join(" ")
    }
}

pub fn clean_cache(binary_dir: &Path) -> Result<()> {
    let cache = binary_dir.join("CMakeCache.txt");
    let files = binary_dir.join("CMakeFiles");
    if cache.exists() {
        std::fs::remove_file(cache)?;
    }
    if files.exists() {
        std::fs::remove_dir_all(files)?;
    }
    Ok(())
}

pub fn run_command(args: &[String], cwd: &Path) -> Result<std::process::Child> {
    if args.is_empty() {
        return Err(Error::Cmake("empty command".into()));
    }
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..]).current_dir(cwd).stdout(std::process::Stdio::piped()).stderr(
        std::process::Stdio::piped(),
    );
    cmd.spawn().map_err(|e| Error::Cmake(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_config_flag() {
        assert!(!Generator::Ninja.needs_config_flag());
        assert!(Generator::VisualStudio.needs_config_flag());
        assert!(Generator::NinjaMultiConfig.needs_config_flag());
        assert!(Generator::Xcode.needs_config_flag());
    }

    #[test]
    fn generator_parse() {
        assert_eq!(Generator::parse("Ninja"), Generator::Ninja);
        assert_eq!(Generator::parse("Ninja Multi-Config"), Generator::NinjaMultiConfig);
        assert_eq!(Generator::parse("Visual Studio 17 2022"), Generator::VisualStudio);
        assert_eq!(Generator::parse("CustomGen"), Generator::Other);
    }

    #[test]
    fn preset_argv() {
        let cmd = ConfigureCommand::Preset {
            name: "tests".into(),
        };
        assert_eq!(cmd.argv(Path::new(".")), vec!["cmake", "--preset", "tests"]);
    }

    #[test]
    fn manual_configure_argv() {
        let cmd = ConfigureCommand::Manual {
            source_dir: PathBuf::from("/src"),
            binary_dir: PathBuf::from("/build"),
            generator: Some("Ninja".into()),
            cache_variables: vec![("WITHOUT_FW".into(), "true".into())],
        };
        assert_eq!(
            cmd.argv(Path::new(".")),
            vec![
                "cmake",
                "-S",
                "/src",
                "-B",
                "/build",
                "-G",
                "Ninja",
                "-DWITHOUT_FW=true",
            ]
        );
    }

    #[test]
    fn for_preset_builds_sysbuild_manual_argv() {
        std::env::set_var("ZEPHYR_BASE", "/opt/zephyr");
        let preset = ResolvedConfigurePreset {
            name: "TRV8-2Full".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-nrfbm"),
            cache_variables: [("CMAKE_BUILD_TYPE".into(), "Debug".into())].into(),
            hidden: true,
        };
        let ov = PresetOverrideConfig {
            source_dir: Some("$env{ZEPHYR_BASE}/share/sysbuild".into()),
            cache_variables: [
                ("APP_DIR".into(), "${sourceDir}".into()),
                (
                    "BOARD".into(),
                    "trv8@1/nrf54l15/cpuapp/s115_softdevice/mcuboot".into(),
                ),
            ]
            .into(),
            ..Default::default()
        };
        let root = Path::new("/proj/tracker-application");
        let binary = root.join("build-nrfbm");
        let cmd = ConfigureCommand::for_preset(&preset, &binary, Some(&ov), root).unwrap();
        let args = cmd.argv(root);
        assert_eq!(
            args,
            vec![
                "cmake".into(),
                "-S".into(),
                "/opt/zephyr/share/sysbuild".into(),
                "-B".into(),
                binary.display().to_string(),
                "-G".into(),
                "Ninja".into(),
                "-DAPP_DIR=/proj/tracker-application".into(),
                "-DBOARD=trv8@1/nrf54l15/cpuapp/s115_softdevice/mcuboot".into(),
                "-DCMAKE_BUILD_TYPE=Debug".into(),
            ]
        );
    }

    #[test]
    fn for_preset_sysbuild_override_can_replace_build_type() {
        std::env::set_var("ZEPHYR_BASE", "/opt/zephyr");
        let preset = ResolvedConfigurePreset {
            name: "TRV8-2Full".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-nrfbm"),
            cache_variables: [("CMAKE_BUILD_TYPE".into(), "Debug".into())].into(),
            hidden: true,
        };
        let ov = PresetOverrideConfig {
            source_dir: Some("$env{ZEPHYR_BASE}/share/sysbuild".into()),
            cache_variables: [
                ("APP_DIR".into(), "${sourceDir}".into()),
                ("CMAKE_BUILD_TYPE".into(), "Release".into()),
            ]
            .into(),
            ..Default::default()
        };
        let root = Path::new("/proj");
        let cmd = ConfigureCommand::for_preset(&preset, &root.join("build-nrfbm"), Some(&ov), root)
            .unwrap();
        let args = cmd.argv(root);
        assert!(args.contains(&"-DCMAKE_BUILD_TYPE=Release".to_string()));
        assert!(!args.contains(&"-DCMAKE_BUILD_TYPE=Debug".to_string()));
    }

    #[test]
    fn for_preset_sysbuild_defaults_build_type_to_debug() {
        std::env::set_var("ZEPHYR_BASE", "/opt/zephyr");
        let preset = ResolvedConfigurePreset {
            name: "BoardX".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build"),
            cache_variables: Default::default(),
            hidden: true,
        };
        let ov = PresetOverrideConfig {
            source_dir: Some("$env{ZEPHYR_BASE}/share/sysbuild".into()),
            cache_variables: [("APP_DIR".into(), "${sourceDir}".into())].into(),
            ..Default::default()
        };
        let root = Path::new("/proj");
        let cmd = ConfigureCommand::for_preset(&preset, &root.join("build"), Some(&ov), root)
            .unwrap();
        let args = cmd.argv(root);
        assert!(args.contains(&"-DCMAKE_BUILD_TYPE=Debug".to_string()));
    }

    #[test]
    fn for_preset_without_override_uses_preset_flag() {
        let preset = ResolvedConfigurePreset {
            name: "tests".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-test"),
            cache_variables: Default::default(),
            hidden: false,
        };
        let cmd = ConfigureCommand::for_preset(
            &preset,
            Path::new("build-test"),
            None,
            Path::new("/proj"),
        )
        .unwrap();
        assert_eq!(cmd.argv(Path::new("/proj")), vec!["cmake", "--preset", "tests"]);
    }

    #[test]
    fn for_preset_empty_override_keeps_preset_flag() {
        let preset = ResolvedConfigurePreset {
            name: "TRV8-2Full".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-nrfbm"),
            cache_variables: Default::default(),
            hidden: true,
        };
        let ov = PresetOverrideConfig::default();
        let cmd = ConfigureCommand::for_preset(
            &preset,
            Path::new("/proj/build-nrfbm"),
            Some(&ov),
            Path::new("/proj"),
        )
        .unwrap();
        assert_eq!(
            cmd.argv(Path::new("/proj")),
            vec!["cmake", "--preset", "TRV8-2Full"]
        );
    }

    #[test]
    fn for_preset_cache_only_override_merges_variables() {
        let preset = ResolvedConfigurePreset {
            name: "tests".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-test"),
            cache_variables: [("FOO".into(), "1".into())].into(),
            hidden: false,
        };
        let ov = PresetOverrideConfig {
            cache_variables: [("BAR".into(), "2".into()), ("FOO".into(), "9".into())].into(),
            ..Default::default()
        };
        let root = Path::new("/proj");
        let cmd = ConfigureCommand::for_preset(&preset, &root.join("build-test"), Some(&ov), root)
            .unwrap();
        let args = cmd.argv(root);
        assert!(args.contains(&"-S".to_string()));
        assert!(args.contains(&"-DBAR=2".to_string()));
        assert!(args.contains(&"-DFOO=9".to_string()));
        assert!(!args.contains(&"-DFOO=1".to_string()));
    }

    #[test]
    fn for_preset_errors_when_env_var_missing() {
        let preset = ResolvedConfigurePreset {
            name: "TRV8-2Full".into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-nrfbm"),
            cache_variables: Default::default(),
            hidden: true,
        };
        let ov = PresetOverrideConfig {
            source_dir: Some("$env{LAZYCMAKE_MISSING_ENV_VAR_XYZ}/share/sysbuild".into()),
            ..Default::default()
        };
        std::env::remove_var("LAZYCMAKE_MISSING_ENV_VAR_XYZ");
        let err = ConfigureCommand::for_preset(
            &preset,
            Path::new("/proj/build-nrfbm"),
            Some(&ov),
            Path::new("/proj"),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("LAZYCMAKE_MISSING_ENV_VAR_XYZ"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_command_without_target_omits_target_flag() {
        let cmd = BuildCommand {
            binary_dir: PathBuf::from("build-nrfbm"),
            target: None,
            clean_first: false,
            generator: Generator::Ninja,
            config: None,
        };
        let jobs = max_job_count().to_string();
        assert_eq!(
            cmd.argv(),
            vec!["cmake", "--build", "build-nrfbm", "--parallel", &jobs]
        );
    }

    #[test]
    fn build_command_argv_with_target_and_clean() {
        let cmd = BuildCommand {
            binary_dir: PathBuf::from("build-test"),
            target: Some("AccelerometerStationaryDetector_run".into()),
            clean_first: true,
            generator: Generator::Ninja,
            config: Some("Debug".into()),
        };
        let jobs = max_job_count().to_string();
        assert_eq!(
            cmd.argv(),
            vec![
                "cmake",
                "--build",
                "build-test",
                "--target",
                "clean",
                "--target",
                "AccelerometerStationaryDetector_run",
                "--parallel",
                &jobs,
            ]
        );
    }

    #[test]
    fn build_command_adds_config_for_multi_config_generators() {
        let cmd = BuildCommand {
            binary_dir: PathBuf::from("build"),
            target: None,
            clean_first: false,
            generator: Generator::VisualStudio,
            config: Some("Release".into()),
        };
        let args = cmd.argv();
        assert!(args.windows(2).any(|w| w == ["--config", "Release"]));
        assert!(args.windows(2).any(|w| {
            w[0] == "--parallel" && w[1] == max_job_count().to_string()
        }));
    }

    #[test]
    fn ensure_parallel_fills_bare_flag_and_respects_explicit_count() {
        let jobs = max_job_count().to_string();
        let mut bare = vec!["ctest".into(), "--parallel".into()];
        ensure_parallel_jobs(&mut bare);
        assert_eq!(bare, vec!["ctest", "--parallel", &jobs]);

        let mut explicit = vec!["ctest".into(), "--parallel".into(), "2".into()];
        ensure_parallel_jobs(&mut explicit);
        assert_eq!(explicit, vec!["ctest", "--parallel", "2"]);

        let mut compact = vec!["ctest".into(), "-j4".into()];
        ensure_parallel_jobs(&mut compact);
        assert_eq!(compact, vec!["ctest", "-j4"]);

        let mut missing = vec!["ctest".into(), "--output-on-failure".into()];
        ensure_parallel_jobs(&mut missing);
        assert_eq!(
            missing,
            vec!["ctest", "--output-on-failure", "--parallel", &jobs]
        );
    }

    #[test]
    fn clean_cache_removes_cache_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("CMakeCache.txt");
        let files = dir.path().join("CMakeFiles");
        std::fs::write(&cache, "x").unwrap();
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("a"), "y").unwrap();

        clean_cache(dir.path()).unwrap();
        assert!(!cache.exists());
        assert!(!files.exists());
    }
}
