STORE ?=
DEMO_STORE ?= demo
PORT ?= 8000
SOURCE ?= .
NAME ?=

store_option = $(if $(STORE),--store "$(STORE)")
name_option = $(if $(NAME),--name "$(NAME)")

.PHONY: serve
serve:
	cargo run --release --locked -- serve $(store_option) --port "$(PORT)"

.PHONY: demo
demo:
	cargo run --release --locked -- serve --store "$(DEMO_STORE)" --port "$(PORT)"

.PHONY: build
build:
	npm run typecheck
	cargo build --release --locked

.PHONY: index
index:
	cargo run --release --locked -- index "$(SOURCE)" $(name_option) $(store_option)

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
