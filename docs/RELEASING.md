# Releasing

## Goal

Ship a tagged GitHub release from hosted compute without relying on local
binary builds.

## Workflow

The repository provides `.github/workflows/release.yml` for release builds.

- Trigger it manually with `workflow_dispatch` from `main`, or
- push a `v*` tag after the version in `Cargo.toml` is ready.

The workflow:

1. verifies that the requested tag matches `Cargo.toml`;
2. rejects reused or force-updated tag events and requires the target commit to
   be contained by `main`;
3. builds standard-runner release bundles for:
   - Linux x86_64
   - Linux arm64
   - macOS arm64
   - macOS x86_64
   - Windows x86_64
4. creates or resumes an exact-tag-commit draft, re-verifies the peeled tag
   commit immediately before publication, and attaches those bundles and
   checksum-verifying install helpers,
   `SHA256SUMS`, `SHA256SUMS.sigstore.json`, and release metadata, then publishes
   it under GitHub's immutable-release policy; and
5. keeps the install path aligned with the tagged source release.

## Expected tag format

The release tag must match the package version exactly:

- package version `0.1.1`
- release tag `v0.1.1`

If the tag and package version drift, the workflow fails before building.

Use GitHub's prerelease flag for alpha tags. Stable releases are published as
the latest release.

## Canonical install paths

Source install from a tagged release:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp --tag v0.1.1 google-ad-manager-mcp
```

Hosted release bundles:

- download the asset that matches your platform from the GitHub release;
- verify it against `SHA256SUMS` and the attached
  `SHA256SUMS.sigstore.json` Sigstore bundle; and
- unpack the archive and place `google-ad-manager-mcp` on your `PATH`.

Example verification flow from the download directory:

```bash
cosign verify-blob SHA256SUMS \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/sednalabs/google-ad-manager-mcp/.github/workflows/release.yml@refs/heads/main" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com"

sha256sum -c SHA256SUMS
```

## Before publishing

Check these first:

1. `main` is green on hosted CI.
2. `Cargo.toml` version is correct.
3. README install instructions still match the workflow outputs.
4. Tool schema and public docs reflect the current surface.
5. The target tag and GitHub release do not already exist; release artifacts
   are never replaced in place.
