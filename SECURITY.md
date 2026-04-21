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

Release artefacts for Linux are signed with the project PGP key. The public key is available in the [`KEYS`](KEYS) file at the root of this repository.

### Import the public key

```sh
gpg --import KEYS
```

Or fetch it directly from the repository:

```sh
curl -sSL https://raw.githubusercontent.com/rayboyd/phase4/main/KEYS | gpg --import
```

### Verify a release artefact

Each signed artefact has a corresponding `.asc` detached signature file included in the release assets.

```sh
gpg --verify phase4-<version>-<target>.tar.gz.asc phase4-<version>-<target>.tar.gz
```

A successful verification will show `Good signature from "Phase4 Release <ray.boyd@pm.me>"`.
