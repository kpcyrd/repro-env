# repro-env

Specify the environment you want to build in like this:

```toml
[container]
image = "rust:1-alpine3.18"
```

If you need a more advanced build environment you can select additional packages like this:

```toml
[container]
image = "docker.io/library/archlinux"

[packages]
system = "archlinux"
dependencies = ["rust-musl", "lua"]
```

## Dogfooding

To avoid the container to interfer with the `target/` directory we develop with, it's recommended to select an explicit target:

```
cargo run -- build -- cargo build --target x86_64-unknown-linux-musl --release
ls -la target/x86_64-unknown-linux-musl/release/repro-env
```
