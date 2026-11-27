/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use anyhow::Result;
use anyhow::bail;

use crate::diff::DiffLine;
use crate::formatln;
use crate::lossy_string;
use crate::newline::BytesNewline;
use crate::newline::SplitLinesByNewline;
use crate::newline::StringNewline;
use crate::outcome::Outcome;
use crate::output::ExitStatus;
use crate::testcase::TestCaseError;
use crate::validation::ValidationFailure;

pub(super) trait OutcomeTestGenerator {
    fn generate_testcase(&self) -> Result<String>;
}

impl Outcome {
    fn generate_testcase_expression(&self) -> String {
        // prepend by command
        let expression_lines = self.testcase.shell_expression.as_bytes();
        let expression_lines = expression_lines.split_at_newline();
        let mut generated = format!("$ {}", lossy_string!(&expression_lines[0].assure_newline()));
        expression_lines.iter().skip(1).for_each(|line| {
            generated.push_str(&format!(
                "> {}",
                lossy_string!(&(&line[..]).assure_newline())
            ))
        });
        generated
    }

    fn generate_testcase_exit_code(&self) -> Option<String> {
        match &self.output.exit_code {
            ExitStatus::Code(code) if *code != 0 => Some(formatln!("[{}]", code)),
            _ => None,
        }
    }
}

impl OutcomeTestGenerator for Outcome {
    fn generate_testcase(&self) -> Result<String> {
        match &self.result {
            Ok(_) => Ok(self.generate_testcase_with_expectations()),

            // The exit code is validated before the output, so whether the
            // output still matches is not known yet.
            Err(TestCaseError::InvalidExitCode { .. }) => {
                match self.testcase.validate_output(&self.output) {
                    Ok(_) => Ok(self.generate_testcase_with_expectations()),
                    Err(err) => self.generate_testcase_from_error(&err),
                }
            }
            Err(err) => self.generate_testcase_from_error(err),
        }
    }
}

impl Outcome {
    /// Generates the testcase with its original expectations and the actual
    /// exit code
    fn generate_testcase_with_expectations(&self) -> String {
        let mut generated = self.generate_testcase_expression();
        self.testcase.expectations().iter().for_each(|expectation| {
            generated.push_str(&expectation.original_string().assure_newline())
        });
        if let Some(exit_code) = self.generate_testcase_exit_code() {
            generated.push_str(&exit_code)
        }
        generated
    }

    fn generate_testcase_from_error(&self, err: &TestCaseError) -> Result<String> {
        match err {
            TestCaseError::ValidationFailed(failure) => match failure {
                ValidationFailure::MalformedOutput(diff) => {
                    let mut generated = self.generate_testcase_expression();

                    // keep matched expectations, output the actual recorded
                    // output lines in place of the unmatched ones
                    for diff_line in diff.lines.iter() {
                        match diff_line {
                            DiffLine::MatchedExpectation {
                                index: _,
                                expectation,
                                lines: _,
                            } => {
                                generated.push_str(&expectation.original_string().assure_newline())
                            }
                            DiffLine::UnexpectedLines { lines } => {
                                for (_, line) in lines {
                                    let suffix = if line.ends_with(b"\n") {
                                        ""
                                    } else {
                                        " (no-eol)"
                                    };
                                    let line = formatln!(
                                        "{}{}",
                                        self.escaping
                                            .escaped_expectation((&line[..]).trim_newlines()),
                                        suffix
                                    );
                                    generated.push_str(&line)
                                }
                            }
                            _ => continue,
                        }
                    }
                    if let Some(exit_code) = self.generate_testcase_exit_code() {
                        generated.push_str(&exit_code)
                    }
                    Ok(generated)
                }
                ValidationFailure::JsonSchemaFailed(_) => {
                    bail!("cannot generate json_schema testcase")
                }
                ValidationFailure::InteractiveFailed(_) => {
                    bail!("cannot generate interactive testcase")
                }
            },
            TestCaseError::InvalidExitCode { .. } => {
                bail!("cannot generate testcase from unvalidated output")
            }
            TestCaseError::InternalError(err) => {
                bail!("cannot generate testcase from internal error: {}", err)
            }
            TestCaseError::Timeout => {
                bail!("cannot generate timed out testcase")
            }
            TestCaseError::Skipped => {
                bail!("cannot generate skipped testcase")
            }
        }
    }
}
