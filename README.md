# monochrome-tui

A terminal client for [monochrome.tf](https://monochrome.tf). Sign in with the
account you already use on the web and listen from your terminal.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/duckesteles/monochrome-tui/main/install.sh | sh
```

Then run `monochrome`.

That is all of it. The script builds in a temporary directory and deletes it
afterwards, so what stays on your machine is one 10 MB program in
`~/.local/bin` and nothing else. If you have no Rust toolchain it borrows one
for the build and removes that too.

You need `git`, `curl` and your distribution's ALSA development package. The
script checks and tells you the exact command if it is missing.

To remove it, along with everything it stored:

```sh
monochrome --uninstall
```

That takes out the settings, the library snapshot, the cache and both tokens
from your system keyring, then the program itself. It lists what it will do and
waits for you to type yes.

<details>
<summary>Building it yourself instead</summary>

```sh
git clone https://github.com/duckesteles/monochrome-tui
cd monochrome-tui
cargo install --path crates/monochrome-tui --root ~/.local
```

`cargo build` leaves several gigabytes in `target/`; `cargo clean` frees it.
</details>

## Signing in

Email and password. Your token is kept in the system keyring.

## Playing music

Search with `/`, press enter on a track. Your saved tracks, playlists and
history are the same ones the web app has, and changes sync back.

The first time you play something, a browser tab opens so the gateway that
serves the audio can run its Cloudflare check. A terminal cannot pass that
check on its own. Clear it in the browser and playback starts.

The result lasts about an hour and survives restarts, so you will rarely see
the screen twice in one sitting.

If you ever get a permanent gateway credential, put it in the config and you
will never see the prompt at all:

```toml
[amazon]
bypass_token = "..."
```

## Keys

Press `?` in the app. The short version:

| | |
| --- | --- |
| `j` `k`, arrows | move |
| `enter` | play or open |
| `esc` | back |
| `/` | search |
| `space` | pause |
| `←` `→` | seek |
| `+` `-` | volume |
| `s` | shuffle |
| `f` | save to library |
| `Q` | quit |

## When something is wrong

```sh
monochrome --doctor              # is the account and gateway working
monochrome --play "artist song"  # does audio work, without opening the app
```

`--doctor` will tell you if your Amazon token has expired, which is the usual
cause of a track refusing to play.

## Where things are kept

```
~/.config/monochrome-tui/config.toml        settings
~/.local/state/monochrome-tui/snapshot.json your library, so the first screen is not empty
~/.local/state/monochrome-tui/credentials   only if your system has no keyring
~/.local/state/monochrome-tui/log           only with --verbose
~/.cache/monochrome-tui/                    the track being played, nothing more
```

All of it is readable only by you.

Nothing you listen to is kept. The track currently playing is buffered to a
file so seeking is instant; that file is unlinked the moment it is opened, so
it never appears in the directory, and the space is released when you move to
the next track. One track at a time, around 15 MB for a four minute lossless
song. Signing out deletes the snapshot and both stored tokens.

## Configuration

`~/.config/monochrome-tui/config.toml`, written on first run. Worth knowing:

```toml
[playback]
quality = "lossless"   # low | high | lossless | hi-res
volume  = 0.7

[ui]
accent  = ""           # a colour name or #rrggbb; empty stays monochrome
spacing = "compact"    # "roomy" puts a blank line between rows
```

The interface never sets a background colour, so it inherits your terminal
theme and transparency as-is.

## What is not here

Visualisers, themes, listening parties, equaliser, podcasts, downloads,
scrobbling, local files, lyrics, OAuth sign-in.

## Credits

Built against the services of
[monochrome-music/monochrome](https://github.com/monochrome-music/monochrome)
(Apache-2.0), which this client shares an account and a library with. None of
its code is used here; see [THIRD-PARTY.md](THIRD-PARTY.md).

## Licence

MIT. Not affiliated with Monochrome, Tidal, Amazon or Deezer.
