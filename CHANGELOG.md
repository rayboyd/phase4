# Changelog

All notable changes to Phase4 will be documented in this file.

## Unreleased

### Bug Fixes

- Ignore non-press key events, e.g keyup which was causing the atomics to flip back off on Windows 11 ([cd12277](cd12277caa8cc2b5b65f8edb267df19c7b646f31))
- Make config imports consistent ([6785604](6785604b08625bd3cc765e432ebd1d0c5a92ded2))
- Correct max_clients doc comment and remove arrows from recorder state machine transitions ([f125a06](f125a065afc0a4eadd9d18447a70f68e15b38ae4))
- Correct .cliffignore commit prefixes ([c883b8b](c883b8b46e91a7ef72f7b5f90a0535e9648ddf79))
- Restore --no-default-features on CI aliases ([e99343f](e99343fe43d67879663f2a2005c9095567146704))
- Bake display-bins-128 into CI aliases ([caf5a71](caf5a7102aef981b286522980a78053c60f7a813))
- Remove sample format from device listing output ([e1821a1](e1821a1d3e3d4b0a7844a373981d0d1bfe8312a6))
- Replace bare unwrap, fix typo, and correct doc comments ([4d239f8](4d239f812e741103ea78048f6d9c7d07f12a5450))

### Build

- Add git-cliff commit exclusion list ([5d36a08](5d36a08e1391b8160bddad419adde61aceb1df0f))

### CI/CD

- Remove docker for now until pinned to 187 correctly ([11b0b79](11b0b793e309a74d5be8c66b30702bbe21a0763a))
- Release workflow ([f4c585f](f4c585f7a807084248a6b3b447d632afc269fae8))

### Documentation

- Add panic doc ([979c226](979c226d02319aca07b7de25af8b1dd4b3988c04))

### Features

- Include build timestamp and target arch in version metadata ([813986a](813986abee8439bdabedf1ec0b22b47935a1a46a))
- Make max client limit configurable ([f62b325](f62b32589003b9d0c9d5d1b1085ade2b2d6e33a1))
- Add channel selection and StreamSink abstraction ([ec45a97](ec45a97aeff74215b1745a4e6a66e9ebd77a9323))
- Add --analyse-channels and --record-channels CLI options ([b8efdb9](b8efdb964cc74e849d88f07487e1692795adf8ff))
- Split release build jobs, gate macos on final tags only ([e8fffaa](e8fffaacb612cf4d7b670fb9444661892a3a61be))

### Refactor

- Rename flag to was_broadcasting_websocket to make intent clearer ([eed92d1](eed92d1bfd7fad7b7dc3e59a6098e7b527e15605))
- Rename flag to is_broadcasting_websocket to make intent clearer ([dcb549f](dcb549f949380d89b1c14720d9949b316ee5405c))
- Extract worker shutdown coordination ([8077933](8077933d9e6ae3843bb4d8bae3051c8372b4a821))
- Introduce ChannelMode, StreamSink, and channel extraction tests ([48b788d](48b788df5ad3c8650dbd2c2bed6b2a5993ac1cf7))
- Split Args into flattened sub-structs with help headings ([570d7d5](570d7d54982b975b4099eaefa9bac933c67b9521))

### Testing

- Add key event kind coverage ([cc2ef8e](cc2ef8e0acc72b994704a28e7bd3bb9111afe3ff))
