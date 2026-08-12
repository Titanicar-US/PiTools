.PHONY: install check check-full quality-gates clean build publish acceptance

PITOOLS_REPOSITORY ?= Titanicar-US/PiTools
PITOOLS_RELEASE_TAG ?=
PITOOLS_PUBLISH_CONFIRM ?=

install:
	cargo fetch --locked
	npm --prefix workers/pi ci

check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets --all-features -- -D warnings
	cargo test --locked
	npm --prefix workers/pi run check
	helm/pitools/ci/verify-render.sh
	bash -n scripts/accept-local-compose.sh
	bash tests/acceptance_script.sh
	bash tests/publish_contract.sh

acceptance:
	bash scripts/accept-local-compose.sh

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
	@PITOOLS_REPOSITORY="$(PITOOLS_REPOSITORY)" \
		PITOOLS_RELEASE_TAG="$(PITOOLS_RELEASE_TAG)" \
		PITOOLS_PUBLISH_CONFIRM="$(PITOOLS_PUBLISH_CONFIRM)" \
		bash scripts/publish-release.sh
