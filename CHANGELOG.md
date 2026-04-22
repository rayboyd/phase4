# Changelog

All notable changes to Phase4 are documented here.

Generated from [Conventional Commits](https://www.conventionalcommits.org/) with [git-cliff](https://github.com/orhun/git-cliff).

## [Unreleased]

### Bug Fixes

- Ignore non-press key events, e.g keyup which was causing the atomics to flip back off on Windows 11 ([`cd12277`](https://github.com/rayboyd/phase4/commit/cd12277caa8cc2b5b65f8edb267df19c7b646f31))

### Build

- Add git-cliff commit exclusion list ([`5d36a08`](https://github.com/rayboyd/phase4/commit/5d36a08e1391b8160bddad419adde61aceb1df0f))

### Documentation

- Update roadmap ([`29883ce`](https://github.com/rayboyd/phase4/commit/29883ce95bf4eddfdb677a858b01db3a594f948f))

### Features

- Include build timestamp and target arch in version metadata ([`813986a`](https://github.com/rayboyd/phase4/commit/813986abee8439bdabedf1ec0b22b47935a1a46a))

### Refactoring

- Rename flag to was_broadcasting_websocket to make intent clearer ([`eed92d1`](https://github.com/rayboyd/phase4/commit/eed92d1bfd7fad7b7dc3e59a6098e7b527e15605))
- Rename flag to is_broadcasting_websocket to make intent clearer ([`dcb549f`](https://github.com/rayboyd/phase4/commit/dcb549f949380d89b1c14720d9949b316ee5405c))

### Testing

- Add key event kind coverage ([`cc2ef8e`](https://github.com/rayboyd/phase4/commit/cc2ef8e0acc72b994704a28e7bd3bb9111afe3ff))
