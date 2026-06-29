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
2. builds standard-runner release bundles for:
   - Linux x86_64
   - macOS arm64
   - Windows x86_64
3. attaches those bundles plus `SHA256SUMS`, `SHA256SUMS.sigstore.json`, and
   release metadata to a GitHub release; and
4. keeps the install path aligned with the tagged source release.

## Expected tag format

The release tag must match the package version exactly:

- package version `0.1.0`
- release tag `v0.1.0`

If the tag and package version drift, the workflow fails before building.

## Canonical install paths

Source install from a tagged release:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp --tag v0.1.0 google-ad-manager-mcp
```

Hosted release bundles:

- download the asset that matches your platform from the GitHub release;
- verify it against `SHA256SUMS` and the attached
  `SHA256SUMS.sigstore.json` Sigstore bundle; and
- unpack the archive and place `google-ad-manager-mcp` on your `PATH`.

## Before publishing

Check these first:

1. `main` is green on hosted CI.
2. `Cargo.toml` version is correct.
3. README install instructions still match the workflow outputs.
4. Tool schema and public docs reflect the current surface.
