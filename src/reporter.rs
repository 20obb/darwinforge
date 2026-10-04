//! User-facing output. Everything diagnostic goes to stderr, so that stdout
//! stays useful for piping (e.g. `darwinforge doctor > report.txt`).

use std::cell::RefCell;

pub struct Reporter {
    verbose: bool,
    warnings: RefCell<Vec<String>>,
}

impl Reporter {
    pub fn new(verbose: bool) -> Self {
        Reporter { verbose, warnings: RefCell::new(Vec::new()) }
    }

    pub fn is_verbose(&self) -> bool {
        self.verbose
    }

    /// A pipeline stage banner, e.g. `==> compile (3 files)`.
    pub fn stage(&self, message: &str) {
        println!("==> {message}");
    }

    /// Ordinary progress output.
    pub fn info(&self, message: &str) {
        println!("{message}");
    }

    /// A skipped/unsupported thing the user should know about. Remembered so the
    /// build can repeat the count at the end.
    pub fn warn(&self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("warning: {message}");
        self.warnings.borrow_mut().push(message);
    }

    /// Echo an external command. Only shown with `-v`, but the child's own
    /// stdout/stderr is *always* inherited and therefore never swallowed.
    pub fn command(&self, program: &str, args: &[String]) {
        if self.verbose {
            eprintln!("+ {program} {}", args.join(" "));
        }
    }

    pub fn warning_count(&self) -> usize {
        self.warnings.borrow().len()
    }
}