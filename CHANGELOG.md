
# Changelog

All notable changes to Phase4 will be documented in this file.

## Unreleased

### Bug Fixes

- Ignore non-press key events, e.g keyup which was causing the atomics to flip back off on Windows 11 ([cd12277](cd12277caa8cc2b5b65f8edb267df19c7b646f31))
- Make config imports consistent ([6785604](6785604b08625bd3cc765e432ebd1d0c5a92ded2))
- Correct max_clients doc comment and remove arrows from recorder state machine transitions ([f125a06](f125a065afc0a4eadd9d18447a70f68e15b38ae4))

### Build

- Add git-cliff commit exclusion list ([5d36a08](5d36a08e1391b8160bddad419adde61aceb1df0f))

### Features

- Include build timestamp and target arch in version metadata ([813986a](813986abee8439bdabedf1ec0b22b47935a1a46a))
- Make max client limit configurable ([f62b325](f62b32589003b9d0c9d5d1b1085ade2b2d6e33a1))

### Refactor

- Rename flag to was_broadcasting_websocket to make intent clearer ([eed92d1](eed92d1bfd7fad7b7dc3e59a6098e7b527e15605))
- Rename flag to is_broadcasting_websocket to make intent clearer ([dcb549f](dcb549f949380d89b1c14720d9949b316ee5405c))

### Testing

- Add key event kind coverage ([cc2ef8e](cc2ef8e0acc72b994704a28e7bd3bb9111afe3ff))

