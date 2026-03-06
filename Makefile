.PHONY: build test clippy dev install-scripts install-layout install-plugin install grant-permissions

all: build install grant-permissions

skills:
	npx skillfish add delorenj/skills zellij-plugin-dev

build:
	cargo build --target wasm32-wasip1 --release

test:
	cargo nextest run --lib

clippy:
	cargo clippy --all-features --lib

dev:
	zellij -l ai-default

install-scripts:
	install -m 755 scripts/zellij-tab-status ~/.local/bin/

install-plugin: build
	install -d ~/.config/zellij/plugins
	install -m 644 target/wasm32-wasip1/release/zjstatus.wasm ~/.config/zellij/plugins/

install-layout:
	install -d ~/.config/zellij/layouts
	install -m 644 layouts/ai-default.kdl ~/.config/zellij/layouts/

install: install-plugin install-layout install-scripts

grant-permissions:
	zellij action launch-or-focus-plugin "file:$$HOME/.config/zellij/plugins/zjstatus.wasm"
	@echo "Now press 'y' in the focused plugin pane to grant permissions"
