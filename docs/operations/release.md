# Release

This document owns durable release gates, not a release calendar or roadmap.

## Supported desktop targets

Primary desktop targets are:

- Windows;
- macOS.

Do not add platform-specific maintenance work for another target without a product decision.

## Release gate

A public build must have:

- passing repository CI at the release candidate commit;
- working clean install on each supported target;
- no known credential-loss regression;
- safe account intake and switching behavior for the advertised auth modes;
- user-visible recovery for supported failure paths;
- only the Tauri permissions required by the application;
- no credential or account-secret leakage in logs;
- accurate README/release notes for current capabilities and limitations.

## Packaging

Use standard Tauri 2 packaging.

Code signing and notarization should follow platform requirements for public distribution.

Do not introduce a custom updater or distribution service merely to ship the first public build.

If an updater is later enabled, use Tauri's signed update mechanism and treat update signing keys as release secrets.

## Privacy

Default posture:

- no analytics;
- no telemetry;
- no listening port;
- no remote-control surface.

Adding collection or crash-reporting requires a separate accepted product/privacy decision.

## Versioning

Use SemVer and keep the authoritative application/package version synchronized during release work.

A merge or passing source build does not by itself mean a public release has been produced.
