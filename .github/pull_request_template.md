<!-- Thanks for contributing. See CONTRIBUTING.md for how to build and test. -->

## What and why

<!-- What does this change, and why? Link the issue it fixes ("Fixes #123"). -->

## How it was tested

<!-- The tests you added or ran. Say whether it was tested on real hardware,
     and which (adapter, firmware version, devices). -->

## Checklist

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] New behavior has a test (simulator, recorded session or unit test)
- [ ] User-visible changes are in `CHANGELOG.md` under `[Unreleased]`
- [ ] Docs updated (README, `docs/`, rustdoc) if behavior, config keys or MQTT topics changed
- [ ] Every commit is signed off (`git commit -s`, see CONTRIBUTING.md)
