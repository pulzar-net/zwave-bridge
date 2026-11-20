# Security policy

zwave-bridge talks to radios that anyone nearby can transmit to, and to an MQTT
broker that other software on the network can reach. We take reports about
either seriously and fix them first.

## Reporting a vulnerability

Report it privately through GitHub:
[**Report a vulnerability**](https://github.com/pulzar-net/zwave-bridge/security/advisories/new)
(the Security tab of this repository, then "Report a vulnerability").

Please do not open a public issue, pull request or discussion for a security
problem, and do not post it anywhere public until a fix is released.

A useful report says:

- which version or commit you tested, and on what hardware and firmware;
- what an attacker needs (radio range, network access, MQTT access, a paired
  device) and what they gain;
- how to reproduce it: a frame capture, a fuzzer input, or a test case is
  ideal.

## What happens next

| Step | Target |
|---|---|
| We acknowledge your report | within 3 business days |
| We confirm or rule out the issue and give a severity | within 10 business days |
| We release a fix for a critical or high severity issue | within 30 days of confirming it |
| We release a fix for a medium or low severity issue | in the next release, within 90 days |

We keep you updated in the advisory as we go, agree a disclosure date with
you, and credit you in the advisory and the changelog unless you prefer not
to be named. We publish a GitHub Security Advisory and request a CVE for
every confirmed vulnerability.

If a problem is in a dependency rather than in this repository, we report it
to that project through its own security process and coordinate with you on
both.

## Supported versions

The project is at 0.x. Security fixes go into the latest release only; we do
not backport to older 0.x releases.

| Version | Supported |
|---|---|
| latest 0.x release | yes |
| older releases | no |
| `main` | yes, fixes land here first |

## Scope

In scope: every crate and binary in this repository, the release binaries,
and the documented defaults.

Out of scope: weaknesses of the Z-Wave specification itself, the MQTT
broker you run, your Home Assistant installation, and deployments that
deliberately turn off a documented protection.

The [security model](https://pulzar-net.github.io/zwave-bridge/security.html)
describes what the project defends against and what it relies on you for.
