//! The `assertion-key` subcommand: generate the Platform assertion seed once and print the public
//! half Edge is configured with (XR-021 CONTRACTS.md section R3-12).
//!
//! The only output on stdout is the base64 public key and one newline, so a script can capture it.
//! The seed is never printed on any stream, and a refusal names the cause without quoting the file.

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use platform_api::assertion_key::{self, AssertionKeyError};

const BINARY: &str = "ratatoskr-telegram-webhook";
const USAGE: &str = "usage: ratatoskr-telegram-webhook assertion-key generate <path> | \
                     assertion-key public <path>";

/// `EX_USAGE`: the command line is wrong.
const EX_USAGE: u8 = 64;

/// Print the usage line to stderr and return the usage exit status.
#[must_use]
pub fn usage() -> ExitCode {
    let _ = writeln!(std::io::stderr(), "{USAGE}");
    ExitCode::from(EX_USAGE)
}

/// Run `assertion-key <action> <path>` over the arguments that follow the subcommand name.
#[must_use]
pub fn run(arguments: &[String]) -> ExitCode {
    let result = match arguments {
        [action, path] if action == "generate" => assertion_key::generate(Path::new(path)),
        [action, path] if action == "public" => assertion_key::public_key_of_file(Path::new(path)),
        _ => return usage(),
    };
    match result {
        Ok(public_key) => {
            // A closed stdout (a broken pipe) is the caller's choice, not a failure of the key.
            let _ = writeln!(std::io::stdout(), "{public_key}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "{BINARY} assertion-key: {}",
                describe(&error)
            );
            ExitCode::FAILURE
        }
    }
}

fn describe(error: &AssertionKeyError) -> String {
    use std::error::Error as _;
    match error.source() {
        Some(source) => format!("{error}: {source}"),
        None => error.to_string(),
    }
}
