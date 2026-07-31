# Security

## Reporting

Use [private vulnerability reporting](https://github.com/duckesteles/monochrome-tui/security/advisories/new).
It reaches the maintainer without the report being public first.

Please do not open a public issue for anything that could expose someone's
account or their listening history.

## What this client holds

Two secrets, both in your system keyring, never in the repository or in any
file this client writes:

- your account token, from signing in
- a gateway token, obtained by a browser check, valid for about an hour

`monochrome --uninstall` removes both, along with the settings, the library
snapshot and the cache.

Error messages and the `--verbose` log run through a redactor before anything
is shown or written. `--doctor` output is meant to be safe to paste into a bug
report: it names no account and prints no address.

## The local browser check

When the audio gateway asks for a browser check, the client opens a listener on
`127.0.0.1` on a random port for at most two minutes. It refuses connections
that are not from loopback, requires a 128 bit nonce on every request, accepts
only `GET`, and closes as soon as it has an answer.

## Scope

The client talks to services it does not control: the monochrome catalog and
the audio gateways. A fault in one of those is not a vulnerability in this
client, though a report is still welcome if this client handles it badly.
