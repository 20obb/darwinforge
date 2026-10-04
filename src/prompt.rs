//! Asking the user questions, safely.
//!
//! Bootstrap must never hang. Every interactive decision goes through here, and
//! every function takes a [`Mode`] that says whether answering is even
//! possible:
//!
//! * [`Mode::Interactive`] — stdin is a TTY: prompt and read a line.
//! * [`Mode::AssumeYes`] — `--yes` was passed: take the default, print what
//!   would have been asked.
//! * [`Mode::AssumeNo`] — no TTY and no `--yes`: **refuse**, with a message
//!   naming the flag that would let the user proceed.
//!
//! The third mode is the important one. A CI job or a piped invocation must get
//! an actionable error in milliseconds, not a prompt that will never be read.

use std::io::{BufRead, IsTerminal, Write};

use crate::error::{Error, Result};

/// How answers are obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// stdin is a TTY: ask.
    Interactive,
    /// `--yes`: accept the default without asking.
    AssumeYes,
    /// No TTY and no `--yes`: refuse rather than hang.
    AssumeNo,
}

impl Mode {
    /// Decide from the flags and whether stdin is a terminal.
    pub fn detect(assume_yes: bool) -> Mode {
        if assume_yes {
            return Mode::AssumeYes;
        }
        if std::io::stdin().is_terminal() {
            Mode::Interactive
        } else {
            Mode::AssumeNo
        }
    }

    /// True when a question would be answered without reading stdin.
    pub fn is_non_interactive(self) -> bool {
        matches!(self, Mode::AssumeYes | Mode::AssumeNo)
    }
}

/// What a prompt is about, used to build the "pass this flag instead" message.
///
/// `subject` is owned rather than `&'static str` because several subjects are
/// only known at run time — "install packages with `apt-get`" is built from the
/// detected manager.
#[derive(Debug, Clone)]
pub struct Question {
    /// What is being decided, e.g. "install packages with apt-get".
    pub subject: String,
    /// The flag that would answer it without a prompt.
    pub flag: &'static str,
}

impl Question {
    /// A question with a compile-time-known subject.
    pub fn new(subject: &'static str, flag: &'static str) -> Question {
        Question { subject: subject.to_string(), flag }
    }

    /// A question whose subject is only known at run time.
    pub fn with_subject(subject: impl Into<String>, flag: &'static str) -> Question {
        Question { subject: subject.into(), flag }
    }
}

/// The error a refused prompt produces.
pub fn needs_interaction(question: &Question) -> Error {
    Error::Setup {
        what: format!("{} needs confirmation, but stdin is not a terminal", question.subject),
        fix: format!(
            "run `darwinforge bootstrap --yes` to accept the defaults, or pass {} \
             explicitly; see `darwinforge bootstrap --help`",
            question.flag
        ),
    }
}

/// Ask a yes/no question.
///
/// In [`Mode::AssumeYes`] the default is taken and the question is echoed so the
/// log still records what was decided. In [`Mode::AssumeNo`] it is an error.
pub fn confirm(mode: Mode, question: &Question, default: bool) -> Result<bool> {
    match mode {
        Mode::AssumeNo => Err(needs_interaction(question)),
        Mode::AssumeYes => {
            println!("  [auto-yes] {} (--yes)", question.subject);
            Ok(default)
        }
        Mode::Interactive => {
            print!(
                "  {} [{}] ",
                question.subject,
                if default { "Y/n" } else { "y/N" }
            );
            std::io::stdout().flush().ok();
            let Some(answer) = read_line() else { return Ok(default) };
            Ok(match answer.trim().to_ascii_lowercase().as_str() {
                "" => default,
                "y" | "yes" => true,
                "n" | "no" => false,
                other => {
                    println!("  `{other}` is not yes or no; using the default ({default})");
                    default
                }
            })
        }
    }
}

/// Ask the user to pick one of `options` by number.
///
/// Returns the chosen index, or `default_index` when non-interactive.
pub fn choose(
    mode: Mode,
    question: &Question,
    options: &[String],
    default_index: usize,
) -> Result<usize> {
    let fallback = default_index.min(options.len().saturating_sub(1));
    match mode {
        Mode::AssumeNo => Err(needs_interaction(question)),
        Mode::AssumeYes => {
            if let Some(label) = options.get(fallback) {
                println!("  [auto-yes] {} -> {label}", question.subject);
            }
            Ok(fallback)
        }
        Mode::Interactive => {
            println!("  {}", question.subject);
            for (index, option) in options.iter().enumerate() {
                let marker = if index == fallback { "*" } else { " " };
                println!("   {marker} {}) {option}", index + 1);
            }
            print!("  choice [{}]: ", fallback + 1);
            std::io::stdout().flush().ok();
            let Some(answer) = read_line() else { return Ok(fallback) };
            let answer = answer.trim();
            if answer.is_empty() {
                return Ok(fallback);
            }
            match answer.parse::<usize>() {
                // 1-based on screen, 0-based internally.
                Ok(number) if (1..=options.len()).contains(&number) => Ok(number - 1),
                _ => {
                    println!("  `{answer}` is not one of the choices; using the default");
                    Ok(fallback)
                }
            }
        }
    }
}

/// Read one line from stdin. `None` at end of input (EOF).
fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(line),
        // A read failure must not be fatal: fall back to the default.
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_never_blocks() {
        let question = Question::new("install packages with apt-get", "--yes");
        assert!(confirm(Mode::AssumeYes, &question, true).expect("must not fail"));
        assert!(!confirm(Mode::AssumeYes, &question, false).expect("must not fail"));
    }

    #[test]
    fn assume_no_refuses_with_an_actionable_error_instead_of_hanging() {
        let question = Question::new("install packages with apt-get", "--yes");
        let error = confirm(Mode::AssumeNo, &question, true).expect_err("must refuse");
        assert_eq!(error.exit_code(), 5, "an incomplete setup is exit code 5");
        let text = error.to_string();
        assert!(text.contains("--yes"), "names the flag that unblocks it: {text}");
        assert!(text.contains("apt-get"), "names what was being asked: {text}");
    }

    #[test]
    fn choose_refuses_when_it_cannot_ask() {
        let question = Question::new("pick an SDK", "--sdk-version");
        let options = vec!["17.5".to_string(), "18.0".to_string()];
        let error = choose(Mode::AssumeNo, &question, &options, 0).expect_err("must refuse");
        assert!(error.to_string().contains("--sdk-version"));
    }

    #[test]
    fn choose_uses_the_default_when_taking_it_as_given() {
        let question = Question::new("pick an SDK", "--sdk-version");
        let options = vec!["17.5".to_string(), "18.0".to_string()];
        assert_eq!(choose(Mode::AssumeYes, &question, &options, 1).expect("ok"), 1);
    }

    #[test]
    fn a_default_index_past_the_end_is_clamped() {
        // Defensive: a stale default must not index out of bounds.
        let question = Question::new("pick an SDK", "--sdk-version");
        let options = vec!["17.5".to_string()];
        assert_eq!(choose(Mode::AssumeYes, &question, &options, 99).expect("ok"), 0);
    }

    #[test]
    fn an_empty_option_list_is_handled() {
        let question = Question::new("pick an SDK", "--sdk-version");
        assert_eq!(choose(Mode::AssumeYes, &question, &[], 0).expect("ok"), 0);
    }

    #[test]
    fn detect_respects_the_assume_yes_flag() {
        // --yes wins regardless of whether stdin is a terminal.
        assert_eq!(Mode::detect(true), Mode::AssumeYes);
    }

    #[test]
    fn both_non_interactive_modes_are_reported_as_such() {
        assert!(Mode::AssumeYes.is_non_interactive());
        assert!(Mode::AssumeNo.is_non_interactive());
        assert!(!Mode::Interactive.is_non_interactive());
    }

    #[test]
    fn every_refusal_names_both_the_subject_and_the_flag() {
        for (subject, flag) in [
            ("install clang", "--yes"),
            ("download an SDK", "--sdk-version"),
            ("build ldid", "--yes"),
        ] {
            let text = needs_interaction(&Question::new(subject, flag)).to_string();
            assert!(text.contains(subject), "missing subject in: {text}");
            assert!(text.contains(flag), "missing flag in: {text}");
        }
    }
}