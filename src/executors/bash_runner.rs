/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use tracing::trace;

use super::context::Context as ExecutionContext;
use super::runner::Runner;
use super::stateful_executor::StatefulExecutorRunnerGenerator;
use super::subprocess_runner::SubprocessRunner;
use crate::output::Output;
use crate::testcase::TestCase;

#[doc = include_str!("./bash_runner.excluded_variables.md")]
pub const BASH_EXCLUDED_VARIABLES: &[&str] = &[
    // variables from Scrut internals
    "__SCRUT_DECLARE_VARS_CMD",
    "__SCRUT_TEMP_STATE_PATH",
    // variables set by scrut in every execution
    "SCRUT_TEST",
    // variables from `man bash`
    "BASHOPTS",
    "BASH_ALIASES",
    "BASH_ARGC",
    "BASH_ARGV",
    "BASH_ARGV0",
    "BASH_CMDS",
    "BASH_COMMAND",
    "BASH_EXECUTION_STRING",
    "BASH_LINENO",
    "BASH_REMATCH",
    "BASH_SOURCE",
    "BASH_SUBSHELL",
    "BASH_VERSINFO",
    "COPROC",
    "DIRSTACK",
    "EUID",
    "FUNCNAME",
    "LINENO",
    "PPID",
    "SHELLOPTS",
    "UID",
];

const BASH_TEMPLATE: &str = include_str!("bash_runner.template");

/// A [`Runner`], that is intended to run a series of contextual related
/// [`crate::executors::execution::Execution`]s, which
/// that ought to share the same environmental context (environment variables, shell
/// variables, shopt, set, functions and aliases).
///
/// It must be initiated with an existing state directory path.
/// After each execution the runner dumps the environmental context in the `state` file of the
/// state directory.
/// Before each execution the runner loads (`source`) the file `state` in the state directory, if
/// it is present.
/// Hence multiple subsequent executions share a consistent environmental context virtually as if
/// they would have been executed from within the same parent bash process (or as close as that is
/// possible without actually running in the same process).
///
/// This Runner is not concurrency-safe (the shared state directory with the `state` file mandates
/// sequential, isolated execution).
///
/// Underneath the [`SubprocessRunner`] is used, so timeout constraints are fully supported.
#[derive(Clone)]
pub struct BashRunner {
    pub shell: PathBuf,
    pub state_directory: PathBuf,
}

impl BashRunner {
    pub fn new(shell: &Path, state_directory: &Path) -> Self {
        Self {
            shell: shell.to_owned(),
            state_directory: state_directory.to_owned(),
        }
    }

    pub fn stateful_generator(shell: &Path) -> StatefulExecutorRunnerGenerator {
        let shell = shell.to_owned();
        Box::new(move |state_directory: &Path| -> Box<dyn Runner> {
            let shell_instance = Self {
                shell: shell.to_owned(),
                state_directory: state_directory.to_owned(),
            };
            Box::new(shell_instance) as Box<dyn Runner>
        })
    }
}

impl Runner for BashRunner {
    fn run(&self, name: &str, testcase: &TestCase, context: &ExecutionContext) -> Result<Output> {
        let shell = self.shell.to_owned();

        let is_detached = testcase.config.detached.unwrap_or(false);
        let restore_environment = self.restore_environment(&testcase.config.environment)?;
        if !is_detached {
            self.persist_configured_environment(&testcase.config.environment)?;
        }

        // render the bash script
        let state_directory_str = self.state_directory.to_string_lossy();
        let expression = BASH_TEMPLATE
            .replace("{state_directory}", &state_directory_str)
            .replace("{name}", name)
            .replace("{restore_environment}", &restore_environment)
            .replace("{shell_expression}", &testcase.shell_expression)
            .replace("{excluded_variables}", &BASH_EXCLUDED_VARIABLES.join("|"))
            .replace("{persist_state}", if is_detached { "0" } else { "1" });
        trace!("compiled expression {}", &expression);

        let mut testcase = testcase.clone();
        testcase.shell_expression = expression;

        let mut output = SubprocessRunner(shell).run(name, &testcase, context)?;

        // read captured environment variables for interpolation support
        let env_path = self.state_directory.join("env");
        if env_path.exists() {
            output.captured_env = parse_env_file(&env_path)?;
        }

        Ok(output)
    }
}

/// File in the state directory that holds the `environment` configuration of
/// the previous (non-detached) testcase.
const CONFIGURED_ENVIRONMENT_FILE: &str = "configured_env";

impl BashRunner {
    /// Renders `export` statements for the configured environment variables
    /// that differ from those of the previous testcase.
    ///
    /// The state of the previous testcase is sourced after the process
    /// environment is set up, so without these statements the inherited values
    /// would replace the ones configured for this testcase. Variables whose
    /// configured value did not change (e.g. those provided by Scrut, or
    /// document defaults) are left alone, so that values a previous testcase
    /// assigned to them are still inherited.
    fn restore_environment(&self, environment: &BTreeMap<String, String>) -> Result<String> {
        let path = self.state_directory.join(CONFIGURED_ENVIRONMENT_FILE);
        let previous = if path.exists() {
            parse_env_file(&path)?
        } else {
            BTreeMap::new()
        };
        Ok(environment
            .iter()
            .filter(|(key, value)| is_shell_identifier(key) && previous.get(*key) != Some(value))
            .map(|(key, value)| format!("export {key}={}\n", shell_quote(value)))
            .collect())
    }

    fn persist_configured_environment(&self, environment: &BTreeMap<String, String>) -> Result<()> {
        let content = environment
            .iter()
            .flat_map(|(key, value)| [key.as_bytes(), b"=", value.as_bytes(), b"\0"])
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        fs::create_dir_all(&self.state_directory)?;
        fs::write(
            self.state_directory.join(CONFIGURED_ENVIRONMENT_FILE),
            content,
        )?;
        Ok(())
    }
}

fn is_shell_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Parse an env file (null-delimited KEY=VALUE entries) into a BTreeMap
fn parse_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let content = fs::read(path)?;
    let is_valid_key =
        |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');

    let env = content
        .split(|&b| b == 0)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| entry.split_once('='))
        .filter(|(key, _)| is_valid_key(key))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();

    Ok(env)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::BashRunner;
    use super::Runner;
    use crate::executors::DEFAULT_SHELL;
    use crate::executors::context::Context as ExecutionContext;
    use crate::output::Output;
    use crate::testcase::TestCase;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_execute_with_timeout_captures_stdout_and_stderr() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let output = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run(
            "name",
            &TestCase::from_expression("echo OK1 && ( 1>&2 echo OK2 )"),
            &ExecutionContext::new_for_test(),
        )
        .expect("execute without error");
        let expect: Output = ("OK1\n", "OK2\n").into();
        assert_eq!(expect, output);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_execute_captures_non_printable_characters() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let output = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run(
            "name",
            &TestCase::from_expression("echo -e \"🦀\r\n😊\""),
            &ExecutionContext::new_for_test(),
        )
        .expect("execute without error");

        let expect: Output = ("🦀\n😊\n", "").into();
        assert_eq!(expect, output);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_execute_with_timeout_captures_exit_code() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let output = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run(
            "name",
            &TestCase::from_expression("( exit 123 )"),
            &ExecutionContext::new_for_test(),
        )
        .expect("execute without error");

        let expect: Output = ("", "", Some(123)).into();
        assert_eq!(expect, output);
    }

    #[test]
    fn test_execute_persists_state_file_in_state_directory() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let _ = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run(
            "name",
            &TestCase::from_expression("true"),
            &ExecutionContext::new_for_test(),
        )
        .expect("execute without error");

        let state_file = temp_dir.path().join("state");
        assert!(
            state_file.exists(),
            "state file was created during execution"
        );
    }

    #[test]
    fn test_detached_execute_does_not_persist_state_file_in_state_directory() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let mut testcase = TestCase::from_expression("true");
        testcase.config.detached = Some(true);
        let context = ExecutionContext::new_for_test();
        let _ = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run("name", &testcase, &context)
        .expect("execute without error");

        // wait until after execution execution
        std::thread::sleep(std::time::Duration::from_millis(300));

        let state_file = temp_dir.path().join("state");
        assert!(
            !state_file.exists(),
            "state file was not created during execution"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_captured_env_contains_exported_variable() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let output = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run(
            "name",
            &TestCase::from_expression("export SCRUT_TEST_VAR=hello_world"),
            &ExecutionContext::new_for_test(),
        )
        .expect("execute without error");
        assert_eq!(
            output.captured_env.get("SCRUT_TEST_VAR"),
            Some(&"hello_world".to_string()),
            "exported variable is captured in env"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_configured_environment_overrides_value_inherited_from_previous_testcase() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let runner = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        };
        let context = ExecutionContext::new_for_test();

        let mut first = TestCase::from_expression("export SCRUT_TEST_VAR=inherited");
        first.config.environment = [("SCRUT_TEST_VAR".into(), "first".into())].into();
        runner
            .run("first", &first, &context)
            .expect("execute without error");

        let mut second = TestCase::from_expression("echo \"$SCRUT_TEST_VAR\"");
        second.config.environment = [("SCRUT_TEST_VAR".into(), "second".into())].into();
        let output = runner
            .run("second", &second, &context)
            .expect("execute without error");

        let expect: Output = ("second\n", "").into();
        assert_eq!(expect, output);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_unchanged_configured_environment_keeps_value_inherited_from_previous_testcase() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let runner = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        };
        let context = ExecutionContext::new_for_test();
        let environment: std::collections::BTreeMap<String, String> =
            [("SCRUT_TEST_VAR".into(), "configured".into())].into();

        let mut first = TestCase::from_expression("export SCRUT_TEST_VAR=inherited");
        first.config.environment = environment.clone();
        runner
            .run("first", &first, &context)
            .expect("execute without error");

        let mut second = TestCase::from_expression("echo \"$SCRUT_TEST_VAR\"");
        second.config.environment = environment;
        let output = runner
            .run("second", &second, &context)
            .expect("execute without error");

        let expect: Output = ("inherited\n", "").into();
        assert_eq!(expect, output);
    }

    #[test]
    fn test_captured_env_is_empty_for_detached() {
        let temp_dir = TempDir::with_prefix("runner.").expect("create temporary directory");
        let mut testcase = TestCase::from_expression("export SCRUT_DETACH_VAR=test");
        testcase.config.detached = Some(true);
        let output = BashRunner {
            shell: DEFAULT_SHELL.to_owned(),
            state_directory: temp_dir.path().into(),
        }
        .run("name", &testcase, &ExecutionContext::new_for_test())
        .expect("execute without error");
        assert!(
            output.captured_env.is_empty(),
            "detached processes do not capture env"
        );
    }
}
