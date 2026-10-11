# assertion-key Specification

## Purpose
Defines how an operator generates the Ed25519 seed that signs Platform identity assertions and derives the base64 public key that Edge is configured with, without the seed ever being printed.

## Requirements

### Requirement: The webhook binary generates the assertion key and derives its public half

`ratatoskr-telegram-webhook assertion-key generate <path>` SHALL create `<path>` exclusively with mode 0600, write a new Ed25519 seed as 64 lowercase hexadecimal characters and one newline, and print the base64 (standard alphabet, padded) public key on one line. `assertion-key public <path>` SHALL read a seed file in the format the services read and print that same key and nothing else on stdout. Neither command SHALL print the seed on any stream, and a malformed seed SHALL be refused without echoing it.

#### Scenario: The RFC 8032 vector

- **WHEN** `assertion-key public` reads a file holding seed `9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60`
- **THEN** stdout is exactly `11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=` and a newline and the exit status is zero

#### Scenario: Generate refuses to overwrite

- **WHEN** `assertion-key generate` runs twice for the same path
- **THEN** the second run exits non-zero and the file is unchanged

#### Scenario: A malformed seed

- **WHEN** `assertion-key public` reads a short, non-hexadecimal or empty file
- **THEN** it exits non-zero and the seed text appears on neither stream

#### Scenario: An unknown subcommand

- **WHEN** the binary receives an unknown first argument or an unknown `assertion-key` action
- **THEN** it exits non-zero without starting the service
