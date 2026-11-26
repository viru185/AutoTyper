# Contributing

Thanks for helping! A few conventions keep the history and the releases tidy.

## Setup

```sh
git clone https://github.com/viru185/AutoTyper
cd AutoTyper
git config core.hooksPath .githooks   # checks commit messages locally
```

You need Rust stable (`rustup`). On Windows also the Visual Studio Build Tools with the
*Desktop development with C++* workload (for the linker and `rc.exe`); on Linux `libxkbcommon-dev`.

## Before you push

```sh
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

CI runs the same on Windows, Linux and macOS.

Changes to the typing engine need a real test as well: type into a real window (Notepad, a
terminal, a Remote Desktop session) with both input methods, and mention what you tested in the PR.

## Commit messages

This project uses [Conventional Commits](https://www.conventionalcommits.org):

```text
<type>(<optional scope>): <description>

feat(gui): add a dark/light theme switch
fix(typing): release Shift when SendInput fails
docs: explain the scan-code mode
```

Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `style`,
`revert`. Add `!` (`feat!: ...`) or a `BREAKING CHANGE:` footer for breaking changes.

Pull requests are squash-merged, and the **PR title** becomes the commit message, so it must
follow the same format (checked by CI).

## Releases

Releases are automated with [release-please](https://github.com/googleapis/release-please):

1. Every push to `main` updates a `chore(main): release x.y.z` pull request with the next
   version (from the commit types) and the changelog.
2. Merging that PR tags the release and creates the GitHub release.
3. The release workflow builds the Windows, Linux and macOS binaries and attaches them.

`feat` bumps the minor version, `fix` and `perf` the patch version, breaking changes the major
version.

Note: pull requests opened by GitHub Actions do not trigger other workflows, so CI does not run
on the release PR itself; everything in it already passed CI on `main`.
