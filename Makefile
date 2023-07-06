build:
	cargo run -- build -- make build2
	sha256sum target/x86_64-unknown-linux-musl/release/repro-env

build2:
	CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
	RUSTFLAGS="-C strip=symbols" \
	cargo build --target x86_64-unknown-linux-musl --release

.PHONY: build build2
