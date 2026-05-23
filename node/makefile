# Caspar node build targets. The Go toolchain isn't required; everything
# is `cargo`.

CARGO ?= cargo

.PHONY: build install dist test clean lint doc casparctl casparctl-install

build:
	$(CARGO) build --release

install: build
	cp target/release/caspar-node     $$HOME/.cargo/bin/caspar-node
	cp target/release/caspar-keygen   $$HOME/.cargo/bin/caspar-keygen

dist:
	@BUILD_TAGS='$(BUILD_TAGS)' $(CURDIR)/scripts/dist.sh $(VERSION)

test:
	$(CARGO) test

lint:
	$(CARGO) clippy --all-targets

doc:
	$(CARGO) doc --no-deps

clean:
	$(CARGO) clean

casparctl:
	$(CARGO) build --release --manifest-path ../cmd/casparctl/Cargo.toml

casparctl-install: casparctl
	cp ../cmd/casparctl/target/release/casparctl $$HOME/.cargo/bin/casparctl
