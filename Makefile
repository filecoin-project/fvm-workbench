# Run the benchmarks, printing the traces
benchmark:
	cargo test --package fvm-workbench-builtin-actors -- --nocapture

build:
	cargo build --workspace

rustfmt:
	cargo fmt

check:
	cargo clippy --workspace