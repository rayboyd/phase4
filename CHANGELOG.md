# Changelog

All notable changes to Phase4 will be documented in this file.

## 0.0.1

[60d58cc](https://github.com/rayboyd/phase4/compare/60d58cc830bb339e78d3d10df628b2d51ca78edd...20ddbfaa9faf6ca90ebd603fcd0ad00c30e1f5a5)

### Bug Fixes

- Update to use correct link for platform requirements and platforms ([56ca1d2](https://github.com/rayboyd/phase4/commit/56ca1d23de51f9ce9e7765faf1e0f5e193006016))
- Correct channel selection wiring and data alignment ([1b386e7](https://github.com/rayboyd/phase4/commit/1b386e7969ba42b026c06a99b1ddef7dc8cc972e))

### Testing

- Test channel selection e2e, issues with config not being passed in and data being scrambled ([1e6ba85](https://github.com/rayboyd/phase4/commit/1e6ba85bff9bdc576c05f83781525215325d3d0d))
- Expand coverage for initialisation and shutdown edge cases ([20ddbfa](https://github.com/rayboyd/phase4/commit/20ddbfaa9faf6ca90ebd603fcd0ad00c30e1f5a5))

## 0.0.0

### Bug Fixes

- Ignore non-press key events, e.g keyup which was causing the atomics to flip back off on Windows 11 ([cd12277](https://github.com/rayboyd/phase4/commit/cd12277caa8cc2b5b65f8edb267df19c7b646f31))
- Make config imports consistent ([6785604](https://github.com/rayboyd/phase4/commit/6785604b08625bd3cc765e432ebd1d0c5a92ded2))
- Correct max_clients doc comment and remove arrows from recorder state machine transitions ([f125a06](https://github.com/rayboyd/phase4/commit/f125a065afc0a4eadd9d18447a70f68e15b38ae4))
- Correct .cliffignore commit prefixes ([c883b8b](https://github.com/rayboyd/phase4/commit/c883b8b46e91a7ef72f7b5f90a0535e9648ddf79))
- Restore --no-default-features on CI aliases ([e99343f](https://github.com/rayboyd/phase4/commit/e99343fe43d67879663f2a2005c9095567146704))
- Bake display-bins-128 into CI aliases ([caf5a71](https://github.com/rayboyd/phase4/commit/caf5a7102aef981b286522980a78053c60f7a813))
- Remove sample format from device listing output ([e1821a1](https://github.com/rayboyd/phase4/commit/e1821a1d3e3d4b0a7844a373981d0d1bfe8312a6))
- Replace bare unwrap, fix typo, and correct doc comments ([4d239f8](https://github.com/rayboyd/phase4/commit/4d239f812e741103ea78048f6d9c7d07f12a5450))
- Add full github urls to commit and compare range links ([e5573f7](https://github.com/rayboyd/phase4/commit/e5573f7238bde6a6b386cf1bd9b496e19b7160ad))
- Auth requests ([a5613a6](https://github.com/rayboyd/phase4/commit/a5613a65287dbdf313bcfc49aa419443816ae274))
- Grant release workflow pull request metadata permissions ([e0d4512](https://github.com/rayboyd/phase4/commit/e0d451263c6d1159759a43728768118389237a32))

### Build

- Add git-cliff commit exclusion list ([5d36a08](https://github.com/rayboyd/phase4/commit/5d36a08e1391b8160bddad419adde61aceb1df0f))

### CI/CD

- Remove docker for now until pinned to 187 correctly ([11b0b79](https://github.com/rayboyd/phase4/commit/11b0b793e309a74d5be8c66b30702bbe21a0763a))
- Release workflow ([f4c585f](https://github.com/rayboyd/phase4/commit/f4c585f7a807084248a6b3b447d632afc269fae8))
- Update gpg keys ([0410f4a](https://github.com/rayboyd/phase4/commit/0410f4a94940b7a20178364a2d064272f9ef4d7a))
- Update key ([24124e3](https://github.com/rayboyd/phase4/commit/24124e34a32527f83660a1ac551a9ec383ce25ae))
- Remove target from cache paths and add restore-keys fallback ([6cc6cfb](https://github.com/rayboyd/phase4/commit/6cc6cfb1369754132bc00f8bc969719bd71b7353))

### Documentation

- Add panic doc ([979c226](https://github.com/rayboyd/phase4/commit/979c226d02319aca07b7de25af8b1dd4b3988c04))
- Update verification instructions to use minisign ([2617cb4](https://github.com/rayboyd/phase4/commit/2617cb451c19f019be380d76ac8cca557a069645))
- Update key verification info ([15c9b9c](https://github.com/rayboyd/phase4/commit/15c9b9cb409be6e3fe60c7bd3defbb44c31c3400))
- Add Phase4 lifecycle diagrams ([163ab05](https://github.com/rayboyd/phase4/commit/163ab057feb899415bc0695a8901edb9d96c2f9a))
- Cleanout editor default styling ([4a3bc6e](https://github.com/rayboyd/phase4/commit/4a3bc6eec9d08cfd9521e799ce0964a51cf371bd))
- Fix bins description, clarify perceptual scale ([f04d4d1](https://github.com/rayboyd/phase4/commit/f04d4d1429be22171fd865545a3d586dd803c62c))
- Add quickstart section and build notes ([c83544e](https://github.com/rayboyd/phase4/commit/c83544ec7879b3c9c82cba2e8bf633561fdc1117))
- Create compile tutorial doc ([6595a7b](https://github.com/rayboyd/phase4/commit/6595a7b00528bcaad4484ea617f66cf8ef0a45d8))
- Fix FFT terminology, improve feature flags table ([be45728](https://github.com/rayboyd/phase4/commit/be45728f929b736b5a3e4ea151833ab336a57111))

### Features

- Include build timestamp and target arch in version metadata ([813986a](https://github.com/rayboyd/phase4/commit/813986abee8439bdabedf1ec0b22b47935a1a46a))
- Make max client limit configurable ([f62b325](https://github.com/rayboyd/phase4/commit/f62b32589003b9d0c9d5d1b1085ade2b2d6e33a1))
- Add channel selection and StreamSink abstraction ([ec45a97](https://github.com/rayboyd/phase4/commit/ec45a97aeff74215b1745a4e6a66e9ebd77a9323))
- Add --analyse-channels and --record-channels CLI options ([b8efdb9](https://github.com/rayboyd/phase4/commit/b8efdb964cc74e849d88f07487e1692795adf8ff))
- Split release build jobs, gate macos on final tags only ([e8fffaa](https://github.com/rayboyd/phase4/commit/e8fffaacb612cf4d7b670fb9444661892a3a61be))
- Surface compiled display-bin count in version output ([7d3967f](https://github.com/rayboyd/phase4/commit/7d3967f129dc7efad568421d7dfe28fbce28ce25))

### Refactor

- Rename flag to was_broadcasting_websocket to make intent clearer ([eed92d1](https://github.com/rayboyd/phase4/commit/eed92d1bfd7fad7b7dc3e59a6098e7b527e15605))
- Rename flag to is_broadcasting_websocket to make intent clearer ([dcb549f](https://github.com/rayboyd/phase4/commit/dcb549f949380d89b1c14720d9949b316ee5405c))
- Extract worker shutdown coordination ([8077933](https://github.com/rayboyd/phase4/commit/8077933d9e6ae3843bb4d8bae3051c8372b4a821))
- Introduce ChannelMode, StreamSink, and channel extraction tests ([48b788d](https://github.com/rayboyd/phase4/commit/48b788df5ad3c8650dbd2c2bed6b2a5993ac1cf7))
- Split Args into flattened sub-structs with help headings ([570d7d5](https://github.com/rayboyd/phase4/commit/570d7d54982b975b4099eaefa9bac933c67b9521))
- Change vocoder defaults ([7c3a6dc](https://github.com/rayboyd/phase4/commit/7c3a6dc247e59e6b10df71a6119006c2804d4782))
- Make standard WS connect and disconnect logs debug level to quieten the term output ([078ab84](https://github.com/rayboyd/phase4/commit/078ab84345fbd60559c625822142af733e67dde3))
- Make the device list a little more helpful for debugging device support, update readme ([1fc386f](https://github.com/rayboyd/phase4/commit/1fc386fd0a0a767b7317680c514f541ecad80083))

### Testing

- Add key event kind coverage ([cc2ef8e](https://github.com/rayboyd/phase4/commit/cc2ef8e0acc72b994704a28e7bd3bb9111afe3ff))
- Update long_version assertion for four-part build metadata ([c6f4ef7](https://github.com/rayboyd/phase4/commit/c6f4ef705fc178c325b00790bc01f99edca8eb07))
