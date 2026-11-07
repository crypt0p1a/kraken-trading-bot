# CI

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml), on every push and pull request:

- **secrets**: [gitleaks](https://github.com/gitleaks/gitleaks) on the whole history and on the
  working tree, with its default rules plus the entropy rules of [`.gitleaks.toml`](../.gitleaks.toml)
  (Kraken private keys and API keys, long high-entropy base64 strings),
  [trufflehog](https://github.com/trufflesecurity/trufflehog) on the history, and no `.env`
  committed
- **lint**: `cargo fmt --check`, clippy with the warnings as errors
- **tests**: the offline suite with [cargo-nextest](https://nexte.st) on Linux x86_64 and
  aarch64, macOS and Windows
- **test report**: the JUnit reports of every platform (`.config/nextest.toml`, profile `ci`)
  published in a *Test results* check
- **coverage**: [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov), summary in the job,
  HTML and lcov reports as artifacts, at least 96% of the lines, nothing left out: the tests run
  the executables, the bot on a mocked Kraken (`KRAKEN_BOT_API_HOST`); sent to Codecov when the
  `CODECOV_TOKEN` secret is set
- **audit**: `cargo audit` of the dependencies
- **screenshots**: the screens of the dashboard on the demo database (`scripts/screenshots.sh`), as
  an artifact: what a change looks like, to compare with `assets/screenshots`
- **docker**: the image of the bot on amd64 and arm64. It must refuse to start without
  credentials, and `--check` must validate the pairs of `managers.example.yaml` on Kraken before
  Kraken refuses its fake key. The cache mounts of the Dockerfile (cargo registry and target) and
  its layers are kept in the cache of GitHub between the runs

The runs of a pull request are cancelled by a new push to it; the runs of `main` always go to the
end, to save the caches the pull requests start from.

Daily and on demand, a tick of the bot runs against the real Kraken with the `KRAKEN_API_KEY` /
`KRAKEN_API_SECRET` secrets of the repository (Settings → Secrets and variables → Actions), its
orders only validated by Kraken, never placed (`live_dry_run`). Without the secrets, the job only
warns.

A secret is also refused before it's committed, by the hook of gitleaks:

```sh
git config core.hooksPath .githooks
```
