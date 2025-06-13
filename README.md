# `deno_package_json`

**ARCHIVED:** This repo was moved into https://github.com/denoland/deno

The package.json implementation used in the Deno CLI.

## Versioning Strategy

This crate does not follow semver so make sure to pin it to a patch version.
Instead a versioning strategy that optimizes for more efficient maintenance is
used:

- Does [deno_config](https://github.com/denoland/deno_config) still compile?
  - If yes, it's a patch release.
  - If no, it's a minor release.
