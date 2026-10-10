# sqlicon-rs

## Coding Guidelines

Follow the Rust coding guidelines in [`doc/coding_rust.md`](doc/coding_rust.md) for every
new or changed line of code in this crate. They cover error handling, panics/`unwrap`,
integer conversions, defensive programming, module/platform layout, logging, testing, and
the Git workflow. When a guideline conflicts with a pattern already used elsewhere in this
codebase, prefer the guideline for new/changed code and leave unrelated existing code
alone unless the task specifically asks for a wider cleanup.

`Cargo.toml` enables the guideline's recommended Clippy lints at `warn` (not `deny`), so
existing code keeps compiling while new violations are visible in `cargo clippy` output.
