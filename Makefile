.PHONY: install check check-full quality-gates clean build publish

install:
	cargo fetch --locked
	npm --prefix workers/pi ci

check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets --all-features -- -D warnings
	cargo test --locked
	npm --prefix workers/pi run check

check-full:
	cargo test --locked --all-features
	npm --prefix workers/pi test

quality-gates:
	cargo audit
	cargo deny check
	npm --prefix workers/pi audit --audit-level=high

clean:
	cargo clean
	npm --prefix workers/pi run clean

build:
	cargo build --release
	npm --prefix workers/pi run build

publish:
	@echo "Publish is intentionally explicit; build artifacts and a registry target are required."
	@false
