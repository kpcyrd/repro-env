# Compile using the host Rust, then run `make repro-env` inside
# the repro-env.lock environment
build:
	cargo run --release -- build -- make repro-env
	sha256sum target/x86_64-unknown-linux-musl/release/repro-env

# Used by the previous step inside the repro-env.lock system
repro-env:
	RUSTFLAGS="-C strip=symbols" \
	cargo build --target x86_64-unknown-linux-musl --release

# Keep `make build2` compatibility around for a while
# Use `make repro-env` in the future
build2: repro-env

docs: docs/repro-env.1

docs/%: docs/%.scd
	scdoc < $^ > $@

.PHONY: build repro-env build2 docs
