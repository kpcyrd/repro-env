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

- **repro-env.toml**: which container image tag you intend to follow (think `Cargo.toml`)
- **repro-env.lock**: which specific image you use for your release build (think `Cargo.lock`)

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

## Download

- [repro-env x86_64 statically linked](https://github.com/kpcyrd/repro-env/releases/download/v0.4.1/repro-env) (sha256: `d6cefae67a91fc42546a2bae904fcb4bb169552229c11a328a0bc0becc212a6d`)

[![](https://repology.org/badge/vertical-allrepos/repro-env.svg)](https://repology.org/project/repro-env/versions)

With github actions:

```yaml
- name: Install repro-env
  run: |
    wget 'https://github.com/kpcyrd/repro-env/releases/download/v0.4.1/repro-env'
    echo 'd6cefae67a91fc42546a2bae904fcb4bb169552229c11a328a0bc0becc212a6d  repro-env' | sha256sum -c -
    sudo install -m755 repro-env -t /usr/bin
```

With github actions (>= Ubuntu 24.04):

```yaml
- name: Install repro-env
  run: sudo apt-get install -y repro-env
```

| Package integration                    | Status | Archive infrastructure |
| -------------------------------------- | ------ | ---------------------- |
| [Arch Linux](#packages-arch-linux)     | ✅ Fully supported, no known issues | ✅ Superb, operated by Arch Linux |
| [Debian](#packages-debian)             | ✅ No known issues | ⚠️ Snapshot service is frequently slow or unavailable |
| [Alpine Linux](#packages-alpine-linux) | ✅ No known issues | ❌ No public archive, links are likely to become 404 |

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

## Packages: Debian

Debian is a widely accepted choice and hosts an archive of all their packages at https://snapshot.debian.org/. You can create a `[packages]` section in your **repro-env.toml** with `system = "debian"` to install additional packages with apt-get.

```toml
# repro-env.toml
[container]
image = "debian:bookworm"

[packages]
system = "debian"
dependencies = ["gcc", "libc6-dev"]
```

Note this only works with **official** debian packages (not ubuntu).

The resolved **repro-env.lock** is going to contain the sha256 of the resolved container image you use as a base, and a list of `[[package]]` that should be installed/upgraded inside of the container before starting the build.

```toml
# repro-env.lock
[container]
image = "debian@sha256:3d868b5eb908155f3784317b3dda2941df87bbbbaa4608f84881de66d9bb297b"

[[package]]
name = "binutils"
version = "2.40-2"
system = "debian"
url = "https://snapshot.debian.org/archive/debian/20230115T211934Z/pool/main/b/binutils/binutils_2.40-2_amd64.deb"
sha256 = "83c3e20b53e1fbd84d764c3ba27d26a0376e361ae5d7fb37120196934dd87424"

[[package]]
name = "binutils-common"
version = "2.40-2"
system = "debian"
url = "https://snapshot.debian.org/archive/debian/20230115T211934Z/pool/main/b/binutils/binutils-common_2.40-2_amd64.deb"
sha256 = "ab314134f43a0891a48f69a9bc33d825da748fa5e0ba2bebb7a5c491b026f1a0"

# [...]
```

## Packages: Alpine Linux

Alpine is very popular in the container world, based on musl libc and has a wide selection of compilers in recent versions. You can create a `[packages]` section in your **repro-env.toml** with `system = "alpine"` to install additional packages with apk. Unfortunately there's currently no public archive of old Alpine packages, you should keep this in mind because your repro-env build environments **are likely to become uninstallable!**

```toml
# repro-env.toml
[container]
image = "docker.io/library/alpine"

[packages]
system = "alpine"
dependencies = ["gcc", "make", "musl-dev"]
```

The resolved **repro-env.lock** is going to contain the sha256 of the resolved container image you use as a base, and a list of `[[package]]` that should be installed/upgraded inside of the container before starting the build.

```toml
# repro-env.lock
[container]
image = "docker.io/library/alpine@sha256:eece025e432126ce23f223450a0326fbebde39cdf496a85d8c016293fc851978"

[[package]]
name = "binutils"
version = "2.40-r7"
system = "alpine"
url = "https://dl-cdn.alpinelinux.org/alpine/v3.18/main/x86_64/binutils-2.40-r7.apk"
sha256 = "6b1bf117b8f0a15862b27ff77a412eaccf2e7d8048a9cc0e3903e44930547c80"

[[package]]
name = "busybox"
version = "1.36.1-r4"
system = "alpine"
url = "https://dl-cdn.alpinelinux.org/alpine/v3.18/main/x86_64/busybox-1.36.1-r4.apk"
sha256 = "abccb59dd5b9e64b782bbfd97b08c79a2214cc53567fb334aa003815505a007f"

# [...]
```

## Bootstrapping

There are no inherent bootstrapping challenges, you can use any recent Rust compiler to build a working **repro-env** binary. This binary can then setup any other build environment (including it's own) and is able to build a bit-for-bit identical copy of the official release binaries hosted on github.

## Reproducible Builds

All [pre-compiled binaries](https://github.com/kpcyrd/repro-env/releases) can be reproduced from source code:

```sh
% wget https://github.com/kpcyrd/repro-env/releases/download/v0.4.1/repro-env
[...]
% sha256sum repro-env
d6cefae67a91fc42546a2bae904fcb4bb169552229c11a328a0bc0becc212a6d  repro-env
```

Since the build environment is fully documented and tracked in git all we need is checkout the corresponding git tag and run `make`:

```sh
% git clone https://github.com/kpcyrd/repro-env
% cd repro-env
% git checkout v0.4.1
% make
% sha256sum target/x86_64-unknown-linux-musl/release/repro-env
d6cefae67a91fc42546a2bae904fcb4bb169552229c11a328a0bc0becc212a6d  target/x86_64-unknown-linux-musl/release/repro-env
```

## License

`MIT OR Apache-2.0`
