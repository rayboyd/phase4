# Changelog

All notable changes to Phase4 will be documented in this file.

## Unreleased

### Documentation

- Remove bare-label comments with no explanatory content, amend lib code layout ([77778b2](https://github.com/rayboyd/phase4/commit/77778b295010c7700d2fe3041768581958f496a3))

### Performance

- Set explicit UDP send buffer size on OSC sender socket ([eaac430](https://github.com/rayboyd/phase4/commit/eaac4307c1d103c3659623be89e7a36e8e77fddd))
- Bundle bin messages into a single UDP packet per frame ([942ad16](https://github.com/rayboyd/phase4/commit/942ad16cefcf86a57223bfde518334f88019842e))

### Refactor

- Park the midi thread rather than sleep ([6ee5a9f](https://github.com/rayboyd/phase4/commit/6ee5a9f36396cf91d27c4834bc28badca01baf1b))

## 0.0.8

### Bug Fixes

- Compute steps in the listener before broadcast, not raw ticks after it ([e93e303](https://github.com/rayboyd/phase4/commit/e93e3035fb09c0501224453589eaad04143b78b8))
- Bring App::new under clippys 100-line pedantic threshold ([de623eb](https://github.com/rayboyd/phase4/commit/de623eb4e3ee89eecc3952b250ee5cd5a5f10dea))
- Use named constants ([3f9d53c](https://github.com/rayboyd/phase4/commit/3f9d53c7fc340bb66e0d2ea91a0bcae4675c960c))
- Resolve device by name before spawning, fail fast like audio ([ccb2298](https://github.com/rayboyd/phase4/commit/ccb22981a1d3a14770c2379d1117cfb397f608c5))

### Documentation

- Reorganise const layout ([54fae74](https://github.com/rayboyd/phase4/commit/54fae74e8431fd63d866e8e40d9bad4dc72f6c0b))
- Split OSC and MIDI into their own Outputs section ([b8a4055](https://github.com/rayboyd/phase4/commit/b8a405598083c81f81c67326e1b1028b0ce32758))

### Features

- Add real device and synthetic clock input, document --midi-device and --midi-test-bpm, update lifecycle diagram ([404b459](https://github.com/rayboyd/phase4/commit/404b45981bc31e12a4d257420aca35f962cb044c))
- Fix help heading and add MIDI device listing ([ce04f3a](https://github.com/rayboyd/phase4/commit/ce04f3a3445d45c0c2c8ec590ff7d5009bab3d51))
- Announce MIDI test clock mode synchronously at startup ([988f7e6](https://github.com/rayboyd/phase4/commit/988f7e6a21d58e860139710921013b80f8a3b1e6))
- Add keyboard transport control for the MIDI test clock ([ac78690](https://github.com/rayboyd/phase4/commit/ac78690cc515d410d5a72badf6bd8634105b832b))
- Forward MIDI transport and steps alongside bin data ([ff98185](https://github.com/rayboyd/phase4/commit/ff98185b0b21e103b6199672176185d91e89da58))

### Refactor

- Prefix audio flags with audio\_, relocate --test-midi-clock to Calibration ([f416902](https://github.com/rayboyd/phase4/commit/f41690242c55d9ed749f16ac39076eda904ef574))
- Relocate transport constants to managers::midi, name the status bytes ([1a95adf](https://github.com/rayboyd/phase4/commit/1a95adf65497f67188fd0a4a2c485f610791d73c))
- Restructure App::new into declare, validate, threads phases ([457aa83](https://github.com/rayboyd/phase4/commit/457aa8312caec29e84eb03ba076d85d96fb0325c))

## 0.0.7

### Bug Fixes

- Reject empty device name instead of fuzzy-matching the first device ([8c2334b](https://github.com/rayboyd/phase4/commit/8c2334bd99a70d1c33d09b82563c0a728bee6c4d))

### Documentation

- Add embedding guide for wrapper processes ([fd7d7c2](https://github.com/rayboyd/phase4/commit/fd7d7c2470be007b9c3de0750865191886b24422))
- Describe the reachable case in the ConfigOutputs comment ([f9c2a04](https://github.com/rayboyd/phase4/commit/f9c2a04f5de9c87287225d39aba1336c8ac694f0))

### Features

- Add display-bins-4, display-bins-8, and display-bins-16 feature flags ([e0722c9](https://github.com/rayboyd/phase4/commit/e0722c9cbf22cd6610149f19c7919eb0f02f7b9a))
- Make WebSocket and OSC output transports opt-in ([af5ac95](https://github.com/rayboyd/phase4/commit/af5ac9502b52142062a8c11264749528f81dfb30))

### Performance

- Skip payload serialisation while no clients are connected ([9c67d73](https://github.com/rayboyd/phase4/commit/9c67d73ce180ab03c6dcf4825fce2bb60931dd6a))

### Testing

- Add integration coverage for the OSC sender's UDP send path ([1280333](https://github.com/rayboyd/phase4/commit/1280333df7d77e7dda32a038cd9f49fc79796c66))

## 0.0.6

### Bug Fixes

- Log startup notes synchronously instead of racing worker threads ([0470097](https://github.com/rayboyd/phase4/commit/04700972bfe5b54a3474e1f91d8b517f0a2802b9))
- Remove startup log line superseded by synchronous logging for WS in app.rs ([1fc8a54](https://github.com/rayboyd/phase4/commit/1fc8a5475037dc6ea814271bedf403454e7d4ef3))
- Remove startup log line superseded by synchronous logging for OSC in app.rs ([320b539](https://github.com/rayboyd/phase4/commit/320b539f43f88d7cbe64bb6f6e4efdcb638f3fc1))
- Reword ready line to close out the startup sequence ([037b53d](https://github.com/rayboyd/phase4/commit/037b53d9a4447decb9664e373429ac542379919d))

### Documentation

- Correct spawn_async_worker fallible setup example to UDP bind ([ad110df](https://github.com/rayboyd/phase4/commit/ad110df258084575f2a8a31cc5864327019eed76))
- Add calibration mode tutorial and flag exclusivity notes ([0d32a5f](https://github.com/rayboyd/phase4/commit/0d32a5f5efb9082feb850ea5fa36ac2ca31774f2))
- Update serve output to match the startup sequence ([6276545](https://github.com/rayboyd/phase4/commit/62765459409d09238ae80c7952dc62287f93791e))

### Features

- Add startup banner in term mode ([acb822c](https://github.com/rayboyd/phase4/commit/acb822c9983d0df1227a5881d907e2a82b0676c6))

### Refactor

- Use the empty payload constant as the snapshot fallback ([2174f8a](https://github.com/rayboyd/phase4/commit/2174f8ae4addb5f25ec9f6899247bde7b5f54749))
- Resolve calibration input into ConfigInput and TestSignal ([39bb04f](https://github.com/rayboyd/phase4/commit/39bb04fec9f923ba0ef3475402b15d8da6f668a2))

## 0.0.5

### Bug Fixes

- Omit carriage return from log lines in headless mode ([f26be8d](https://github.com/rayboyd/phase4/commit/f26be8d2b246935d88a18a3aacb9ae77122797a4))
- Send unconnected so no-listener packets are silently dropped ([7301153](https://github.com/rayboyd/phase4/commit/7301153df75ee76e5948b5df1f55043a1320add6))

### Build

- Scope rt-multi-thread tokio feature to dev-dependencies ([74c8c35](https://github.com/rayboyd/phase4/commit/74c8c35b102ecce8c0d362dd2d0153db5d153f3a))

### CI/CD

- Test the default display-bins-32 configuration ([535b046](https://github.com/rayboyd/phase4/commit/535b04636f9314f48be049179831b2f18ebcdfb5))

### Documentation

- Correct config.yaml location to current working directory ([742dc6f](https://github.com/rayboyd/phase4/commit/742dc6f94d27bb7840fa3c35bb276febdf747dc2))
- Replace Resolume example with TouchDesigner OSC In CHOP ([12bcbf0](https://github.com/rayboyd/phase4/commit/12bcbf0e0beba3395eca52ec8b56e343f288061c))
- Replace Resolume integration section with TouchDesigner ([9d5f67c](https://github.com/rayboyd/phase4/commit/9d5f67cfc729e474bde4503195c1ce6cb453647b))

### Refactor

- Route initial snapshot through payload validation ([c18a8bd](https://github.com/rayboyd/phase4/commit/c18a8bdc0590f1d791c39b9eaad751f9a0baf5fb))
- Resolve InputSource once instead of re-deriving calibration_mode ([682784a](https://github.com/rayboyd/phase4/commit/682784a6416540b111d97b4014ccb8df14a095c1))

## 0.0.4

### Bug Fixes

- Restore terminal on panic before process abort ([71635f0](https://github.com/rayboyd/phase4/commit/71635f088eb5bd5c5448c5cfb6d6f9d492533c7f))
- Reject non-finite DisplayPayload values and rate-limit log spam ([a12bc99](https://github.com/rayboyd/phase4/commit/a12bc99609d326afa300208016c8877cd8fcacb7))

### Features

- Add explicit --controller-mode flag and headless controller ([f79da47](https://github.com/rayboyd/phase4/commit/f79da47b90f136724457689bb89f6a220f0dc1ce))
- Add JSON output format for --list ([4c696c7](https://github.com/rayboyd/phase4/commit/4c696c7dc966d79c4f12100b066e1fb1d299739a))

### Performance

- Reduce ACCEPT_TIMEOUT_MS to match Controller::POLL_RATE_MS ([7817880](https://github.com/rayboyd/phase4/commit/7817880de336c08a7dc911e7f65a5b2e94c0d313))

### Refactor

- Extract spawn_async_worker helper for tokio worker threads ([1873b7f](https://github.com/rayboyd/phase4/commit/1873b7fb26f6a6dd8a1ab672133ae687350bc6aa))
- Structure AppConfigError variants instead of carrying strings ([0f24d52](https://github.com/rayboyd/phase4/commit/0f24d52d67917ef6e91efc8c6a7893dd6488b4ff))
- Standardise main() on anyhow::Result ([8db9382](https://github.com/rayboyd/phase4/commit/8db93821a45f674d7445c2017f27f9621b6e0352))

## 0.0.3

### Bug Fixes

- Use DEFAULT_BROADCAST_RATE_HZ ([08eb227](https://github.com/rayboyd/phase4/commit/08eb2279d959b90d517bd83ca5e48bce2599788f))
- Address code review findings across all priority levels ([52b2bdf](https://github.com/rayboyd/phase4/commit/52b2bdf1b5d7c7f664d357694fd071a03873b28f))
- Remove default input device fallback ([56de9d7](https://github.com/rayboyd/phase4/commit/56de9d74fb094e062fcd515676aca774338e76e7))

### Documentation

- Correct allocation claim after sender refactor ([ad36b83](https://github.com/rayboyd/phase4/commit/ad36b83e482e914d400fa17854091c471b920123))
- Update data-flow diagram for single typed display channel ([ebeac2b](https://github.com/rayboyd/phase4/commit/ebeac2ba334f8e45c7cce90ade8b00f48bc4445a))

### Features

- Add centre_frequencies ([d9e9305](https://github.com/rayboyd/phase4/commit/d9e93050c2471ce3ab39eced844f0f15f956b21a))
- Centralise display payload fanout and server side serialisation ([f93a59c](https://github.com/rayboyd/phase4/commit/f93a59c007ee7694fb6dd9f7a0567dfae5f9eb94))

### Performance

- Serialise display payload without a per-frame clone ([41ad5e8](https://github.com/rayboyd/phase4/commit/41ad5e87bda9970339044132b2aa301bc3d1e7ae))

### Refactor

- Eliminate per-frame allocations and hoist runtime state ([145f633](https://github.com/rayboyd/phase4/commit/145f633f5dc957a298e9693924650a01de26d27e))
- Seed serialised channel and drop first-frame bootstrap ([39fd904](https://github.com/rayboyd/phase4/commit/39fd904c08c03f670a206843504c8fd593765d4b))
- Drop unused enumerate in the send loop ([58134f1](https://github.com/rayboyd/phase4/commit/58134f1da83c104330bfc401fd2b93440580ae81))

### Testing

- Move wire-contract test beside the serialize impl ([da2aa63](https://github.com/rayboyd/phase4/commit/da2aa63309c7a9e352a249cfc7deb45c3d2d3ade))

## 0.0.2

### Bug Fixes

- Correct --osc flag reference to --osc-addr ([6a8e4be](https://github.com/rayboyd/phase4/commit/6a8e4be65358994830ee6d69992fb24d8d554a47))
- Validate channel indices against hardware capacity at startup ([2d8b298](https://github.com/rayboyd/phase4/commit/2d8b29850702f410fa7872cdabd662a39c6fbb09))
- Change toggle key binding from A/B to T ([de11663](https://github.com/rayboyd/phase4/commit/de11663e4bd43c4de2f108b57448837d1290b65d))

### Documentation

- Add module and public API doc comments ([448b252](https://github.com/rayboyd/phase4/commit/448b252fdc63601e9fb55063cc4af25cad4725cb))
- Add OSC tutorial, update README and lifecycle diagrams ([ad88086](https://github.com/rayboyd/phase4/commit/ad88086b12cb52cab9e39cf3a2589185fcb58000))
- Remove all recording references from docs and inline comments ([55033cd](https://github.com/rayboyd/phase4/commit/55033cd7e0b5655745a6c877699e1ccb8131e0e2))
- Add example.config.yaml with all supported keys and defaults ([db882ad](https://github.com/rayboyd/phase4/commit/db882ad82554324db25b2e3225b4471e8c7bbb1b))
- Update readme, lifecycle diagram, and tutorials for config.yaml support ([4e94a35](https://github.com/rayboyd/phase4/commit/4e94a3528125167d005d73f4039f29c3b040f12f))

### Features

- Introduce Milliseconds and Hertz newtypes for VocoderConfig, add compile_fail doctests and inner value roundtrip tests ([e588073](https://github.com/rayboyd/phase4/commit/e588073b32fe6abeeaf4856efd96f510f85b0420))
- Default broadcast rate to 30 Hz ([0dbb788](https://github.com/rayboyd/phase4/commit/0dbb788193fd07cc534596f792fb8f0a78e7050c))
- Add UDP OSC output via --osc flag and change default display bins to 32 ([21c59a6](https://github.com/rayboyd/phase4/commit/21c59a64c56606b54887322fc253829e65599d50))
- Add YAML file config layer with three-tier merge (CLI > file > default) ([3818ee4](https://github.com/rayboyd/phase4/commit/3818ee4bfa3eb226d9fe316c3e525ba88ddf719f))

### Refactor

- Split worker code into worker module ([a0b60a8](https://github.com/rayboyd/phase4/commit/a0b60a83afeae495934d58f5be042ed83ee7387b))
- Clippy point-free refactor ([1f1f1a3](https://github.com/rayboyd/phase4/commit/1f1f1a3c47beca3fd689c37f3a2a735fdbcba7a6))
- Remove recording thread and all associated infrastructure ([b6e7e7e](https://github.com/rayboyd/phase4/commit/b6e7e7ecf5950e2772a9ec7addc16b6e0be4a22b))
- Eliminate per-frame heap allocation in OscSender hot path ([09b9a53](https://github.com/rayboyd/phase4/commit/09b9a53d89f7c70c72a55172f86fd588826c7167))
- Consolidate is_analysing and is_broadcasting_websocket into single is_active flag ([01813ae](https://github.com/rayboyd/phase4/commit/01813ae0fdd6ccd2d48be6ac438a9a435799254b))
- Replace device index with name-based 3-tier resolution ([f2caa9f](https://github.com/rayboyd/phase4/commit/f2caa9f9e5c6554b35fc981c8f2c702b2897aa66))

### Testing

- Test_ringbuf_power_of_two ([f3c25b3](https://github.com/rayboyd/phase4/commit/f3c25b3cc3681417d0e0e4ebdfeb297f3f5cffdd))

## 0.0.1

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
