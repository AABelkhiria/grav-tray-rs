.PHONY: build run test package publish-dry-run clean

build:
	cargo build --release

run:
	cargo run --release

test:
	cargo test --all-targets
	cargo clippy --all-targets -- -D warnings

package:
	cargo package

publish-dry-run:
	cargo publish --dry-run

clean:
	cargo clean
