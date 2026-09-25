ARTIFACT_STORE ?= artifacts
DEMO_STORE ?= demo
PORT ?= 8000
SOURCE ?=
NAME ?= $(notdir $(abspath $(SOURCE)))

.PHONY: serve
serve:
	cargo run --release --locked -- serve --store "$(ARTIFACT_STORE)" --port "$(PORT)"

.PHONY: demo
demo:
	cargo run --release --locked -- serve --store "$(DEMO_STORE)" --port "$(PORT)"

.PHONY: build
build:
	npm run typecheck
	cargo build --release --locked

.PHONY: index
index:
	@test -n "$(SOURCE)" || (echo 'Set SOURCE to the codebase directory.' >&2; exit 1)
	cargo run --release --locked -- "$(SOURCE)" --name "$(NAME)" --store "$(ARTIFACT_STORE)"

.PHONY: test
test:
	cargo test --locked --all-targets
	npm test

.PHONY: check
check:
	cargo fmt --check
	cargo clippy --locked --all-targets --all-features
	npm run typecheck
	$(MAKE) test
