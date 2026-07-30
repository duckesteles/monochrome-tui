# Third-party licences

This client links 299 Rust crates. Every one of them is licensed, none are
copyleft beyond the file level, and none restrict commercial or private use.

## Monochrome

This client speaks to the same services as
[monochrome-music/monochrome](https://github.com/monochrome-music/monochrome),
which is Apache-2.0, Copyright 2026 Monochrome Team.

No code from that project is present here. Its source was read to learn the
service endpoints, the shape of the stored library document, and the parameters
of the encryption scheme the audio gateway uses, all of which had to match
exactly for the two clients to share an account. Endpoint addresses and wire
formats are facts about an interface rather than expression, so this is not a
derivative work and the project is licensed independently. The attribution is
here because the project would not exist without theirs.

Monochrome ships no NOTICE file, so there is none to reproduce.

## Mozilla Public License 2.0

The audio decoding stack is MPL-2.0 (17 crates). That licence is file-level
copyleft: it is satisfied by linking the crates unmodified, which is what happens
here, and by saying where the source is. None of these files have been modified.

Sixteen of them are Symphonia, the decoding stack itself.

Source: https://github.com/pdeljanov/Symphonia

- symphonia, symphonia-bundle-flac, symphonia-bundle-mp3, symphonia-codec-aac, symphonia-codec-adpcm, symphonia-codec-alac, symphonia-codec-pcm, symphonia-codec-vorbis, symphonia-common, symphonia-core, symphonia-format-caf, symphonia-format-isomp4, symphonia-format-mkv, symphonia-format-ogg, symphonia-format-riff, symphonia-metadata

The seventeenth arrives with `dirs`, which is how this client finds your
config and cache directories.

Source: https://github.com/soc/option-ext

- option-ext

## Everything else

Permissive, and where a choice is offered this project takes the MIT or
Apache-2.0 option:

- MIT OR Apache-2.0 — 155
- MIT — 61
- Apache-2.0 OR MIT — 24
- Unicode-3.0 — 18
- Apache-2.0 OR ISC OR MIT — 3
- MIT/Apache-2.0 — 3
- Apache-2.0 — 2
- Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT — 2
- Apache-2.0/MIT — 2
- ISC — 2
- (MIT OR Apache-2.0) AND Unicode-3.0 — 1
- 0BSD OR MIT OR Apache-2.0 — 1
- Apache-2.0 / MIT — 1
- Apache-2.0 AND ISC — 1
- Apache-2.0 OR BSL-1.0 — 1
- BSD-3-Clause — 1
- MIT OR Zlib OR Apache-2.0 — 1
- Unlicense OR MIT — 1
- Zlib — 1
- Zlib OR Apache-2.0 OR MIT — 1

## Distributing a build

Source distribution carries no further obligation. If you publish a compiled
binary, ship this file with it: the MIT, Apache-2.0, BSD, ISC, Zlib and
Unicode-3.0 licences all ask that their notices travel with the binary, and
MPL-2.0 asks that recipients are told where to get the source.

Run `cargo metadata` for the full list with versions and repositories.

