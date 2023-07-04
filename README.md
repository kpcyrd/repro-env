# repro-env

## Dogfooding

To avoid the container to interfer with the `target/` directory we develop with it's recommended to select an explicit target:

```
cargo run -- build -- cargo build --target x86_64-unknown-linux-musl --release
ls -la target/x86_64-unknown-linux-musl/release/repro-env
```
