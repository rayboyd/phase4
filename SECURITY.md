# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| latest  | Yes       |

Only the latest release is actively supported with security updates.

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly.

**Do not open a public issue.**

- **GitHub:** [Private vulnerability reporting](https://github.com/rayboyd/phase4/security/advisories/new)

## Verifying Signatures

Release artefacts are signed with [minisign](https://jedisct1.github.io/minisign/). The public key is available in the [`minisign.pub`](minisign.pub) file at the root of this repository.

### Verify a release artefact

Each signed artefact has a corresponding `.minisig` signature file included in the release assets.

```sh
minisign -V -p minisign.pub -m phase4-<version>-<target>.tar.gz
```

A successful verification will show `Signature and comment signature verified`.

Install minisign via your package manager (`brew install minisign`, `apt install minisign`, etc.) or from the [minisign releases page](https://github.com/jedisct1/minisign/releases).
