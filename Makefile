build:
	cargo run --release -- build -- make build2
	sha256sum target/x86_64-unknown-linux-musl/release/repro-env

build2:
	CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
	RUSTFLAGS="-C strip=symbols" \
	cargo build --target x86_64-unknown-linux-musl --release

docs: docs/repro-env.1

docs/%: docs/%.scd
	scdoc < $^ > $@

.PHONY: build build2 docs
