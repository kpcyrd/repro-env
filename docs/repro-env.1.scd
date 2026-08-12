REPRO-ENV(1)

# NAME

repro-env - Dependency lockfiles for reproducible build environments

# SYNOPSIS

*repro-env* update

*repro-env* build -- [_COMMAND_]

# DESCRIPTION

Tracks a description of a desired state in *repro-env.toml*, for example, the latest version of some official container image, with the latest patch level and the latest version of some additional packages.
It also tracks a resolved variant in *repro-env.lock* that tracks the specific versions and checksums of the packages needed to set up the described environment (at the patch-level available at the time of writing).

The purpose of *repro-env.toml* is to make it trivial to re-resolve the specification if new patches become available. This file is read by *repro-env update*.

The purpose of *repro-env.lock* is to document which compiler versions have been used for the release binary associated with a given release. Recording this information is essential for reproducible builds and allows future forensic investigation of the build environment. It can be either committed into the source-code repository or attached to a release as an artifact, along with the compiled binary. This file is read by *repro-env build*.

# GLOBAL OPTIONS

*-v*, *--verbose*
	Increase logging output (can be used multiple times)

*-C* _path_, *--context* _path_
	Change the current directory to this path before executing the subcommand

# UPDATE

This command resolves the environment described in *repro-env.toml* with the latest available updates and writes a *repro-env.lock*. You would use this command similar to how you would use *cargo update*.

*--no-pull*
	Do not attempt to pull the container tag from registry before resolving it

*-k*, *--keep*
	Do not delete the build container, wait for ctrl-c

# BUILD

This command loads a *repro-env.lock*, sets up the environment it describes in a container and mounts the current directory to */build* inside of the container. It then runs the given _COMMAND_ inside of this container.

*-f* _path_, --file _path_
	The dependency lockfile to use

*-k*, *--keep*
	Do not delete the build container, wait for ctrl-c

*-e* _env_, **--env** _env_
	Pass environment variables into the build container (FOO=bar or just FOO to lookup the value)

# PACKAGES: ARCH LINUX

Arch Linux hosts a comprehensive collection of recent compilers at https://archive.archlinux.org. You can create a *[packages]* section in your *repro-env.toml* with *system = "archlinux"* to install additional packages with pacman.

```
# repro-env.toml
[container]
image = "docker.io/library/archlinux"

[packages]
system = "archlinux"
dependencies = ["rust-musl", "lua"]
```

# PACKAGES: DEBIAN

Debian is a widely accepted choice and hosts an archive of all their packages at https://snapshot.debian.org/. You can create a *[packages]* section in your *repro-env.toml* with *system = "debian"* to install additional packages with apt-get.

```
# repro-env.toml
[container]
image = "debian:bookworm"

[packages]
system = "debian"
dependencies = ["gcc", "libc6-dev"]
```

# BOOTSTRAP COMMAND

You can declare bootstrap command(s) to run right after the container creation (before dependencies are installed and the build command is executed) with *bootstrap_cmd = [["command", "arg"]]* in the *[container]* section of your *repro-env.toml*. You can pass multiple commands and arguments with *bootstrap_cmd = [["command1", "arg1", "arg2"], ["command2", "arg1", "arg2", "arg3"]]*.

```
# repro-env.toml
[container]
image = "docker.io/archlinux/archlinux:repro"
bootstrap_cmd = [["pacman-key", "--init"], ["pacman-key", "--populate", "archlinux"]]

[packages]
system = "archlinux"
dependencies = ["cargo", "musl", "rust-musl"]
```

# AUTHORS

repro-env is developed on github at https://github.com/kpcyrd/repro-env
