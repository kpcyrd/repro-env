# repro-env

Imagine you had a tool that takes a config like this:

```toml
# repro-env.toml
[container]
image = "rust:1-alpine3.18"
```

and turns it into something like this:

```toml
# repro-env.lock
[container]
image = "rust@sha256:22760a18d52be83a74f5df8b190b8e9baa1e6ce7d9bda40630acc8ba5328a2fd"
```

You commit both into your git repository to document:

- **repro-env.toml**: which docker image tag you intend to follow (like `Cargo.toml`)
- **repro-env.lock**: which specific image you use for your release build (like `Cargo.lock`)

The .lock file is auto-generated and can be refreshed with a simple command:

```
repro-env update
```

The build is executed in a user-namespace with **podman** (make sure it's installed), the current directory is mounted to `/build/` and a given command is executed inside of that directory:

```
repro-env build -- cargo build
```

We want to distribute our binary without having to worry about system libraries, so we ask cargo to create static binaries (also enable release optimizations):

```
repro-env build -- cargo build --release --target x86_64-unknown-linux-musl
```

This way we also ensure a different build folder is used (`target/x86_64-unknown-linux-musl` instead of `target/`) so our normal development doesn't interfere.

The final executable is available at this location:

```
./target/x86_64-unknown-linux-musl/release/repro-env --help
```

## Packages: Arch Linux

Arch Linux hosts a comprehensive collection of recent compilers at https://archive.archlinux.org. You can create a `[packages]` section in your **repro-env.toml** with `system = "archlinux"` to install additional packages with pacman.

```toml
# repro-env.toml
[container]
image = "docker.io/library/archlinux"

[packages]
system = "archlinux"
dependencies = ["rust-musl", "lua"]
```

The resolved **repro-env.lock** is going to contain the sha256 of the resolved container image you use as a base, and a list of `[[package]]` that should be installed/upgraded inside of the container before starting the build.

```toml
# repro-env.lock
[container]
image = "docker.io/library/archlinux@sha256:6568d3f1f278827a4a7d8537f80c2ae36982829a0c6bccff4cec081774025472"

# [...]

[[package]]
name = "rust"
version = "1:1.69.0-3"
system = "archlinux"
url = "https://archive.archlinux.org/packages/r/rust/rust-1%3A1.69.0-3-x86_64.pkg.tar.zst"
sha256 = "b8eb31a2eb80efab27bb68beab80436ed3e1d235a217c3e24ba973936c95839e"
signature = "iIsEABYIADMWIQQGaHodnU+rCLUP2Ss7lKgOUKR3xwUCZExVKBUcaGVmdGlnQGFyY2hsaW51eC5vcmcACgkQO5SoDlCkd8fQkAD6AudRi2qP3WxSn38OOkSRSITciqRevPaVJgrz03JUBEAA/12h9z8dReD07Lqnltx9QTa3Cxppbv7VpJlTCQuavoMG"

[[package]]
name = "rust-musl"
version = "1:1.69.0-3"
system = "archlinux"
url = "https://archive.archlinux.org/packages/r/rust-musl/rust-musl-1%3A1.69.0-3-x86_64.pkg.tar.zst"
sha256 = "5a4854cdac8312dbf72fb87795bcc36bfb34e9218944966e5ac2e62319bbcf22"
signature = "iIsEABYIADMWIQQGaHodnU+rCLUP2Ss7lKgOUKR3xwUCZExVKRUcaGVmdGlnQGFyY2hsaW51eC5vcmcACgkQO5SoDlCkd8cCMQD/W59RkOVPZDXlnmyY27jW61GC86hXOkSLOKa7XMQtpBoBALSugCkG1clSo/EQDbnuS+UY3268HNBvz6mF6i/hhEsB"
```

## Bootstrapping

There are no inherent bootstrapping challenges, you can use any recent Rust compiler to build a working **repro-env** binary. This binary can then setup any other build environment and is able to build a bit-for-bit identical copy of the official release binaries hosted on github.

## Reproducible Builds

All pre-compiled binaries can be reproduced from source code.

```
% wget https://github.com/kpcyrd/repro-env/releases/download/v0.1.0/repro-env
[...]
% sha256sum repro-env
5b7e043dea9c2a0afc0180be9263dd5c5b7e69c649749b43c132885e4eca623f  repro-env
```

Since the build environment is fully documented and tracked in git all we need is checkout the corresponding git tag and run `make`:

```sh
% git clone https://github.com/kpcyrd/repro-env
% cd repro-env
% git checkout v0.1.0
% make
% sha256sum target/x86_64-unknown-linux-musl/release/repro-env
5b7e043dea9c2a0afc0180be9263dd5c5b7e69c649749b43c132885e4eca623f  target/x86_64-unknown-linux-musl/release/repro-env
```

## License

`GPL-3.0-or-later`
